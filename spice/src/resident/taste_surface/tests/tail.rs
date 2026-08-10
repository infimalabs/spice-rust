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
    assert!(
        failure.contains("steer::inbox-unwritable"),
        "parked failure: {failure}"
    );
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

    fs::remove_dir_all(blocked_rendering).expect("fixture unblocks inbox rendering");
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

#[test]
fn emitted_reminders_ack_decline_escalate_and_expire_as_ordinary_steering() {
    let fixture = Fixture::new("ordinary-steering");
    let bag = bag(&[
        ("polling", &["poll*"], "Wait."),
        ("fallbacks", &["fallback*"], "Keep one path."),
    ]);
    fixture.recognize(
        &bag,
        &prose_record(fixture.lane, fixture.occupant, "polling fallback"),
    );
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    let reminders = reminders(&snapshot);
    let mut disposed = steer::Ledger::from_snapshot(&snapshot).expect("ledger");
    disposed
        .absorb(steer::Steering::Acked(&wire::body::SteerAcked {
            directive: reminders[0].directive,
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            epoch: fixture.epoch,
            locus: reminders[0].locus,
        }))
        .expect("semantic ACK retires reminder");
    disposed
        .absorb(steer::Steering::Declined(&wire::body::SteerDeclined {
            directive: reminders[1].directive,
            key: reminders[1].key.clone(),
            lane: fixture.lane,
            epoch: fixture.epoch,
            locus: reminders[1].locus,
            reason: "not applicable".to_owned(),
        }))
        .expect("semantic NACK retires reminder");
    assert_eq!(
        disposed.disposition(&reminders[0].key.parse().expect("key")),
        Some(Disposition::Acked)
    );
    assert_eq!(
        disposed.disposition(&reminders[1].key.parse().expect("key")),
        Some(Disposition::Declined)
    );

    let mut lifecycle = steer::Ledger::from_snapshot(&snapshot).expect("ledger");
    lifecycle
        .absorb(steer::Steering::Escalated(&wire::body::SteerEscalated {
            directive: reminders[0].directive,
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            stage: wire::body::SteeringStage::Reminded,
        }))
        .expect("reminder escalates");
    lifecycle
        .absorb(steer::Steering::Lapsed(&wire::body::SteerLapsed {
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            expires: reminders[0].expires,
        }))
        .expect("stable reminder expires");
    assert_eq!(
        lifecycle.disposition(&reminders[0].key.parse().expect("key")),
        Some(Disposition::Lapsed)
    );
    fixture.finish();
}

fn bag(rows: &[(&str, &[&str], &str)]) -> taste::Bag {
    taste::Bag::new(
        rows.iter()
            .map(|(name, words, message)| {
                taste::Taste::new(
                    name.parse().expect("taste name"),
                    words.iter().copied(),
                    *message,
                )
                .expect("taste row")
            })
            .collect(),
    )
    .expect("taste bag")
}

fn prose_record(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    text: &str,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(7, 120, text.len() as u64);
    record(
        lane,
        occupant,
        listen::Decoded::Events {
            context: Default::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: Some("final".to_owned()),
                media: Vec::new(),
                locus,
            })],
        },
    )
}

fn record(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decoded: listen::Decoded,
) -> lane::capture::CaptureRecord {
    lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus: listen::Locus::new(7, 120, 64),
        declaration: None,
        decoded,
    }
}

fn reminders(snapshot: &journal::FactSnapshot) -> Vec<wire::body::TasteReminded> {
    snapshot
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read())
        .collect()
}

fn judgments(snapshot: &journal::FactSnapshot) -> Vec<wire::body::TasteJudged> {
    snapshot
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read())
        .collect()
}

fn judgment(confirmed: bool, attempts: Vec<wire::body::TasteJudgeAttempt>) -> Judgment {
    Judgment {
        confirmed,
        attempts,
    }
}

fn attempt(
    framing: u8,
    reply: &str,
    verdict: wire::body::TasteJudgeReply,
) -> wire::body::TasteJudgeAttempt {
    wire::body::TasteJudgeAttempt {
        framing,
        reply: reply.to_owned(),
        verdict,
    }
}

fn append_send(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    key: &str,
    cause: Seq,
) {
    let at = current_instant().expect("command instant");
    let origin: wire::Origin = format!("resident:{epoch}").parse().expect("command origin");
    let mut proposal =
        JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let body = wire::body::SteerSent {
        directive,
        key: key.to_owned(),
        lane,
        text: "concurrent command".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(cause))
        .expect("command fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("command head")
    });
    commit(bus, proposal).expect("concurrent command commits");
}

fn append_compaction(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    cause: Seq,
) -> Seq {
    let mut proposal =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("compaction proposal");
    proposal
        .fact(
            wire::body::LaneCompacted {
                lane,
                occupant,
                stream: "stdout".to_owned(),
                member: 12,
                offset: 500,
                length: 32,
            },
            FactCause::Existing(cause),
        )
        .expect("compaction fact");
    Seq::new(commit(bus, proposal).expect("compaction commits").last()).expect("nonzero compaction")
}

fn append_reminder(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    candidate: &Candidate,
) {
    let at = current_instant().expect("reminder instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("reminder origin");
    let mut proposal = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        at,
        Producer::ResidentRecognition,
    );
    let body = wire::body::TasteReminded {
        directive: uuid(70),
        key: "00000000".to_owned(),
        lane,
        taste: candidate.taste.clone(),
        content: candidate.content.clone(),
        occupant,
        compaction: anchor.get(),
        locus: candidate.locus,
        text: candidate.text.clone(),
        expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(anchor))
        .expect("reminder fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::reminded_head(&body, origin, at, seq).expect("reminder head")
    });
    commit(bus, proposal).expect("concurrent reminder commits");
}

fn append_judgment(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    opening: &Opening,
) {
    let at = current_instant().expect("judgment instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("judgment origin");
    let mut proposal = JournalWrite::new(Actor::Spice, origin.clone(), at, Producer::Judge);
    let body = wire::body::TasteJudged {
        lane,
        occupant,
        compaction: anchor.get(),
        taste: opening.candidate.taste.clone(),
        content: opening.candidate.content.clone(),
        bag: opening.candidate.bag.clone(),
        trigger: opening.candidate.trigger.clone(),
        vendor: opening.candidate.vendor.clone(),
        source: opening.candidate.source.clone(),
        locus: opening.candidate.locus,
        attempts: vec![attempt(0, "YES", wire::body::TasteJudgeReply::Yes)],
        decision: wire::body::TasteDecision::Confirmed {
            directive: opening.directive,
            key: opening.key.to_string(),
            text: opening.candidate.text.clone(),
            expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
        },
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(anchor))
        .expect("judgment fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::judged_head(&body, origin, at, seq).expect("judgment head")
    });
    commit(bus, proposal).expect("concurrent judgment commits");
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}
