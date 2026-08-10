fn cancellation_path(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "cancel this direction"],
        false,
    ));
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    record_reply(
        fixture,
        lane.parse().expect("lane UUID"),
        &format!("ACK {}: too late", sent.key),
        &sent.key,
    );
    let refused = invoke_as_lane(fixture, &lane, &["ack", &sent.key], true);
    assert_refusal(&refused, "steer::ack-missed");
}

fn direct_input_cannot_ack(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "require durable transcript evidence"],
        false,
    ));
    let refused = invoke_as_lane(fixture, &lane, &["ack", &sent.key], false);
    assert_text_refusal(&refused, "steer::transcript-evidence-absent");
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive),
        "direct ack input retired a directive without transcript evidence"
    );
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
}

fn rendering_failure_path(
    fixture: &Fixture,
    resident: RunningResident,
    lane: wire::CanonicalUuid,
    historical_key: &str,
) {
    // ST-05 + ID-10 + CON-05: the journal commit remains authoritative when
    // its disposable inbox rendering fails, and the failure is observable.
    let lane = lane.to_string();
    let inbox_root = fixture.runtime.join("inbox");
    let inbox_path = inbox_root.join(&lane);
    fs::remove_dir_all(&inbox_root).expect("fixture removes disposable inbox renderings");
    fs::create_dir_all(&inbox_path).expect("fixture blocks the lane inbox publication");
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "remain authoritative without a rendering"],
        false,
    ));
    let wire::intent::DeliveryStanding::Parked {
        source,
        keys,
        failure,
        repair,
    } = sent.delivery.as_ref()
    else {
        panic!("failed publication did not return parked delivery: {sent:?}");
    };
    assert!(source.get() > 0);
    assert_eq!(keys, std::slice::from_ref(&sent.key));
    assert!(failure.contains("steer::inbox-unwritable"));
    assert_eq!(repair, &format!("spice steer repair {lane}"));
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive)
    );
    assert_eq!(&inbox.lanes[0].delivery, sent.delivery.as_ref());

    stop(fixture, resident);
    let mut restarted = RunningResident::spawn(fixture);
    restarted.assert_ready(fixture);
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive),
        "restart retired the parked operator directive"
    );
    let wire::intent::DeliveryStanding::Parked {
        source: replayed_source,
        keys: replayed_keys,
        failure: replayed_failure,
        repair: replayed_repair,
    } = &inbox.lanes[0].delivery
    else {
        panic!("startup replay did not retain parked operator delivery: {inbox:?}");
    };
    assert!(
        replayed_source >= source,
        "startup replay regressed the parked source cursor"
    );
    assert_eq!(replayed_keys, keys);
    assert!(replayed_failure.contains("steer::inbox-unwritable"));
    assert!(replayed_failure.contains("inbox rendering"));
    assert_eq!(replayed_repair, repair);
    let duplicate = invoke_as_lane(fixture, &lane, &["ack", historical_key], true);
    assert_refusal(&duplicate, "steer::ack-missed");

    fs::remove_dir_all(&inbox_path).expect("fixture unblocks inbox publication");
    let repaired = invoke(fixture, &["steer", "repair", &lane], true);
    let Outcome::Reply(repaired): Outcome<wire::intent::DeliveryAnswer> = json_outcome(&repaired)
    else {
        panic!("fold repair must reply: {:?}", repaired.stderr);
    };
    assert_eq!(repaired.body.lane, lane.parse().expect("lane UUID"));
    assert!(matches!(
        repaired.body.delivery.as_ref(),
        wire::intent::DeliveryStanding::Published { .. }
    ));
    let rendered = fs::read_to_string(&inbox_path).expect("repair republishes inbox");
    assert!(rendered.contains(&sent.key));
    assert_eq!(rendered.matches(&sent.key).count(), 1);
    let repaired_again = invoke(fixture, &["steer", "repair", &lane], true);
    let Outcome::Reply(repaired_again): Outcome<wire::intent::DeliveryAnswer> =
        json_outcome(&repaired_again)
    else {
        panic!(
            "idempotent fold repair must reply: {:?}",
            repaired_again.stderr
        );
    };
    assert_eq!(repaired_again.body.delivery, repaired.body.delivery);
    assert_eq!(
        fs::read_to_string(&inbox_path).expect("repeat repair leaves inbox readable"),
        rendered,
        "ID-10 repair is idempotent at an unchanged journal horizon"
    );
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    stop(fixture, restarted);
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal): Outcome<wire::intent::DirectiveAnswer> = json_outcome(output)
    else {
        panic!("command must refuse: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason(), reason);
}

fn assert_text_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    assert!(output.stdout.is_empty());
    let text = std::str::from_utf8(&output.stderr).expect("text refusal is UTF-8");
    assert!(text.starts_with(&format!("spice: {reason};")), "{text:?}");
}

fn verify_authority(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    race: wire::Disposition,
    expected: &[ExpectedRecognition],
) {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    journal.verify().expect("steering authority verifies");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    let count = |kind| {
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == kind)
            .count()
    };
    assert_eq!(count(FactKind::SteerSent), 6);
    assert_eq!(
        count(FactKind::SteerAcked),
        1 + usize::from(race == wire::Disposition::Acked)
    );
    assert_eq!(count(FactKind::SteerDeclined), 1);
    assert_eq!(
        count(FactKind::SteerCanceled),
        3 + usize::from(race == wire::Disposition::Canceled)
    );
    assert!(snapshot.facts().iter().all(|fact| {
        !matches!(
            fact.fact.kind(),
            FactKind::SteerAcked | FactKind::SteerDeclined
        ) || matches!(fact.actor, Actor::Lane(named) if named == lane)
    }));
    let mut observed = Vec::new();
    for envelope in snapshot.facts() {
        if let Some(body) = envelope.fact.read::<wire::body::SteerAcked>() {
            observed.push(ExpectedRecognition {
                directive: body.directive,
                key: body.key,
                epoch: body.epoch,
                disposition: wire::Disposition::Acked,
                locus: body.locus,
            });
        } else if let Some(body) = envelope.fact.read::<wire::body::SteerDeclined>() {
            observed.push(ExpectedRecognition {
                directive: body.directive,
                key: body.key,
                epoch: body.epoch,
                disposition: wire::Disposition::Declined,
                locus: body.locus,
            });
        }
    }
    assert_eq!(observed.len(), expected.len());
    for expected in expected {
        assert!(observed.iter().any(|observed| {
            observed.directive == expected.directive
                && observed.key == expected.key
                && observed.epoch == expected.epoch
                && observed.disposition == expected.disposition
                && observed.locus == expected.locus
        }));
        assert_transcript_locus(fixture, lane, expected.locus, &expected.key);
    }
    let missed = snapshot
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::SteerMissed>())
        .collect::<Vec<_>>();
    assert!(
        missed.len() >= 2,
        "duplicate and historical ACKs were not recorded"
    );
    let initial_epoch = expected.first().expect("successful recognition").epoch;
    assert!(
        missed.iter().all(|body| {
            body.lane == lane
                && body.miss == wire::body::AcknowledgmentMiss::Historical
                && body.epoch == initial_epoch
        }),
        "unexpected missed recognitions: {missed:#?}"
    );
    let unique = missed
        .iter()
        .map(|body| (&body.key, body.locus))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        missed.len(),
        "a missed recognition was duplicated"
    );
    for body in &missed {
        assert_transcript_locus(fixture, lane, body.locus, &body.key);
    }
}

fn directive_authored(fixture: &Fixture, directive: wire::CanonicalUuid) -> Instant {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    snapshot
        .facts()
        .iter()
        .find_map(|envelope| {
            let authored = envelope
                .fact
                .read::<wire::body::SteerSent>()
                .map(|body| body.directive)
                .or_else(|| {
                    envelope
                        .fact
                        .read::<wire::body::TasteReminded>()
                        .map(|body| body.directive)
                })?;
            (authored == directive).then_some(envelope.at)
        })
        .expect("pending directive has an authoring fact")
}

fn assert_transcript_locus(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    locus: wire::body::TranscriptLocus,
    token: &str,
) {
    let tape = tape::Tape::open_existing(fixture.tape(lane)).expect("lane tape reopens");
    let bytes = tape
        .read(tape::Locus::new(locus.member, locus.offset, locus.length))
        .expect("persisted transcript locus resolves");
    let decoded = listen::decode(
        listen::Dialect::Claude,
        &bytes,
        tape::Locus::new(locus.member, locus.offset, locus.length),
    )
    .expect("persisted transcript locus is exact");
    let event = decoded
        .events()
        .get(usize::try_from(locus.event).expect("event index fits"))
        .expect("persisted transcript event exists");
    let listen::Event::Prose(prose) = event else {
        panic!("persisted transcript event is not assistant prose");
    };
    let line = prose
        .text
        .lines()
        .nth(usize::try_from(locus.line - 1).expect("line index fits"))
        .expect("persisted transcript line exists");
    let column = usize::try_from(locus.column - 1).expect("column index fits");
    assert_eq!(line.get(column..column + token.len()), Some(token));
}

fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

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
