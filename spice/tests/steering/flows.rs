use super::*;

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
    let publication_block = InboxPermissionBlock::new(inbox_root.clone());
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "remain authoritative without a rendering"],
        false,
    ));
    assert_eq!(
        sent.delivery.as_ref(),
        &wire::intent::DeliveryStanding::Unknown
    );
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane], false));
    let wire::intent::DeliveryStanding::Parked {
        source,
        keys,
        failure,
        repair,
    } = &inbox.lanes[0].delivery
    else {
        panic!("post-response reconciliation did not park delivery: {inbox:?}");
    };
    assert!(source.get() > 0);
    assert_eq!(keys, std::slice::from_ref(&sent.key));
    assert!(failure.contains("steer::inbox-unwritable"));
    assert_eq!(repair, &format!("spice steer repair {lane}"));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive)
    );

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
    assert!(
        replayed_failure.contains("steer::inbox-unwritable"),
        "restart changed the parked failure class: {replayed_failure}"
    );
    assert_eq!(replayed_repair, repair);
    let duplicate = invoke_as_lane(fixture, &lane, &["ack", historical_key], true);
    assert_refusal(&duplicate, "steer::ack-missed");

    publication_block.restore();
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
    let rendered = fs::read_to_string(inbox_root.join(&lane)).expect("repair republishes inbox");
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
        fs::read_to_string(inbox_root.join(&lane)).expect("repeat repair leaves inbox readable"),
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

/// The ack command captured as its own tool call is not the acknowledgment.
///
/// A vendor persists the assistant text spoken before a tool call as its own
/// record ahead of the tool record, so a header spoken as visible prose retires
/// the key while the occupant is still seated. A header that reaches the tape
/// only inside the tool input names the key without anyone having said it, and
/// the key stays live until prose does.
fn same_message_tool_call_cannot_ack(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
) -> ExpectedRecognition {
    let lane_id = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane_id, "answer before the turn ends"],
        false,
    ));
    let epoch = lane_epoch(fixture, lane);
    record_tool_call(fixture, lane, &sent.key);
    let refused = invoke_as_lane(fixture, &lane_id, &["ack", &sent.key], false);
    assert_text_refusal(&refused, "steer::transcript-evidence-absent");
    let inbox = text_inbox(&invoke(fixture, &["inbox", &lane_id], false));
    assert!(
        inbox.lanes[0]
            .pending
            .iter()
            .any(|pending| pending.directive == sent.directive),
        "a tool input naming the header retired a directive nobody spoke"
    );
    let locus = record_reply(
        fixture,
        lane,
        &format!("ACK {}: spoken before the turn ended", sent.key),
        &sent.key,
    );
    let acknowledged = text_directive(&invoke_as_lane(
        fixture,
        &lane_id,
        &["ack", &sent.key],
        false,
    ));
    assert_eq!(acknowledged.directive, sent.directive);
    assert_eq!(acknowledged.disposition, wire::Disposition::Acked);
    ExpectedRecognition {
        directive: sent.directive,
        key: sent.key,
        epoch,
        disposition: wire::Disposition::Acked,
        locus,
    }
}

/// Capture the ack command as the only record of one assistant message, with
/// the header spelled inside its tool input and nowhere else.
fn record_tool_call(fixture: &Fixture, lane: wire::CanonicalUuid, key: &str) {
    let record = serde_json::json!({
        "type": "assistant",
        "message": {
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_fixture_ack",
                "name": "Bash",
                "input": {
                    "command": format!("spice ack {key}"),
                    "description": format!("ACK {key}: applied inside the tool call alone")
                }
            }]
        }
    });
    let mut bytes = wire::encode(&record).expect("tool call transcript record encodes");
    bytes.push(b'\n');
    let tape = tape::Tape::open(fixture.tape(lane)).expect("lane tape opens");
    tape.tee(&bytes).expect("tool call record reaches tape");
}

pub(super) fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal): Outcome<wire::intent::DirectiveAnswer> = json_outcome(output)
    else {
        panic!("command must refuse: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason(), reason);
}

pub(super) fn assert_text_refusal(output: &Output, reason: &str) {
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
    assert_eq!(count(FactKind::SteerSent), 7);
    assert_eq!(
        count(FactKind::SteerAcked),
        2 + usize::from(race == wire::Disposition::Acked)
    );
    assert_eq!(count(FactKind::SteerDeclined), 1);
    assert_eq!(
        count(FactKind::SteerCanceled),
        3 + usize::from(race == wire::Disposition::Canceled)
    );
    verify_derived_attribution(&snapshot, lane);
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

/// Who wrote each derived steering fact, and what it descends from.
///
/// Lane-authenticated recognitions name the lane as actor. Origin names the
/// chain root, not the writer: every derived steering fact cites its
/// steer::sent as cause and repeats the origin that fact minted. The directive
/// canceled after the restart in rendering_failure_path is what separates this
/// from the epoch of the resident writing the link.
fn verify_derived_attribution(snapshot: &journal::FactSnapshot, lane: wire::CanonicalUuid) {
    assert!(snapshot.facts().iter().all(|fact| {
        !matches!(
            fact.fact.kind(),
            FactKind::SteerAcked | FactKind::SteerDeclined
        ) || matches!(fact.actor, Actor::Lane(named) if named == lane)
    }));
    let by_seq = snapshot
        .facts()
        .iter()
        .map(|fact| (fact.seq, fact))
        .collect::<std::collections::BTreeMap<_, _>>();
    for fact in snapshot.facts() {
        if !matches!(
            fact.fact.kind(),
            FactKind::SteerAcked
                | FactKind::SteerDeclined
                | FactKind::SteerCanceled
                | FactKind::SteerEscalated
        ) {
            continue;
        }
        let cause = fact
            .cause
            .expect("derived steering fact cites its directive");
        let root = by_seq[&cause];
        assert_eq!(root.fact.kind(), FactKind::SteerSent, "{fact:#?}");
        assert_eq!(fact.origin, root.origin, "{fact:#?}");
    }
}

pub(super) fn directive_authored(fixture: &Fixture, directive: wire::CanonicalUuid) -> Instant {
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

pub(super) fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

pub(super) fn instant(value: &str) -> Instant {
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
    assert_eq!(resent.directive, sent.directive);
    assert_eq!(resent.key, sent.key);
    assert_eq!(resent.authored, sent.authored);
    assert_eq!(resent.disposition, sent.disposition);
    assert_eq!(
        sent.delivery.as_ref(),
        &wire::intent::DeliveryStanding::Unknown
    );
    assert!(matches!(
        resent.delivery.as_ref(),
        wire::intent::DeliveryStanding::Published { .. }
    ));

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
fn transcript_ack_persists_the_tape_generation_source() {
    let (fixture, resident, lanes) = setup();
    let lane = lanes.target.to_string();
    let sent = text_directive(&invoke(
        &fixture,
        &["steer", &lane, "persist the transcript source"],
        false,
    ));
    let expected = record_reply(
        &fixture,
        lanes.target,
        &format!("ACK {}: source captured", sent.key),
        &sent.key,
    );
    let acknowledged = text_directive(&invoke_as_lane(&fixture, &lane, &["ack", &sent.key], false));
    assert_eq!(acknowledged.disposition, wire::Disposition::Acked);

    let facts = journal::Journal::open(fixture.journal())
        .expect("steering journal opens")
        .fact_snapshot()
        .expect("steering facts read");
    let observed = facts
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::SteerAcked>())
        .find(|body| body.directive == sent.directive)
        .expect("acknowledgment fact is durable");
    assert_eq!(observed.locus, expected);
    let source = tape::Tape::open_existing(fixture.tape(lanes.target))
        .expect("lane tape opens")
        .source_id()
        .expect("lane tape source reads")
        .as_str()
        .parse()
        .expect("tape source is canonical");
    assert_eq!(observed.locus.source, Some(source));
    stop(&fixture, resident);
}

fn shared_canvas_fanout(
    fixture: &Fixture,
    lanes: Lanes,
    correction: &str,
) -> (
    wire::CanonicalUuid,
    wire::intent::DirectiveAnswer,
    wire::intent::DirectiveAnswer,
) {
    let fanout = uuid("00000000-0000-4000-8000-000000000080");
    let (mut socket, discovery) = operator_socket(fixture);
    let target = send_operator_direction(
        &mut socket,
        &discovery,
        uuid("00000000-0000-4000-8000-000000000081"),
        fanout,
        lanes.target,
        current_occupant(fixture, lanes.target),
        correction,
    );
    let other = send_operator_direction(
        &mut socket,
        &discovery,
        uuid("00000000-0000-4000-8000-000000000082"),
        fanout,
        lanes.other,
        current_occupant(fixture, lanes.other),
        correction,
    );
    drop(socket);

    let target_locus = acknowledge(fixture, lanes.target, &target);
    let other_locus = acknowledge(fixture, lanes.other, &other);
    assert_ne!(target_locus.source, other_locus.source);
    assert_eq!(
        (
            target_locus.member,
            target_locus.offset,
            target_locus.length,
            target_locus.event,
            target_locus.line,
            target_locus.column,
        ),
        (
            other_locus.member,
            other_locus.offset,
            other_locus.length,
            other_locus.event,
            other_locus.line,
            other_locus.column,
        ),
        "separate real tapes deliberately share local coordinates"
    );
    (fanout, target, other)
}

fn acknowledge(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    sent: &wire::intent::DirectiveAnswer,
) -> wire::body::TranscriptLocus {
    let locus = record_reply(
        fixture,
        lane,
        &format!("ACK {}: applied", sent.key),
        &sent.key,
    );
    let acknowledged = text_directive(&invoke_as_lane(
        fixture,
        &lane.to_string(),
        &["ack", &sent.key],
        false,
    ));
    assert_eq!(acknowledged.disposition, wire::Disposition::Acked);
    locus
}

fn distinct_cli_fanouts(
    fixture: &Fixture,
    lanes: Lanes,
    correction: &str,
) -> (wire::intent::DirectiveAnswer, wire::intent::DirectiveAnswer) {
    let target_lane = lanes.target.to_string();
    let other_lane = lanes.other.to_string();
    let target = text_directive(&invoke(
        fixture,
        &["steer", &target_lane, correction],
        false,
    ));
    let other = text_directive(&invoke(fixture, &["steer", &other_lane, correction], false));
    acknowledge(fixture, lanes.target, &target);
    acknowledge(fixture, lanes.other, &other);
    (target, other)
}

fn assert_fanout_authority(
    fixture: &Fixture,
    fanout: wire::CanonicalUuid,
    target: &wire::intent::DirectiveAnswer,
    other: &wire::intent::DirectiveAnswer,
    cli_target: &wire::intent::DirectiveAnswer,
    cli_other: &wire::intent::DirectiveAnswer,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let facts = journal.fact_snapshot().expect("steering facts read");
    let sent_fanout = |directive| {
        facts
            .facts()
            .iter()
            .filter_map(|fact| fact.fact.read::<wire::body::SteerSent>())
            .find(|sent| sent.directive == directive)
            .expect("operator direction is durable")
            .fanout
            .expect("current operator direction carries fanout")
    };
    assert_eq!(sent_fanout(target.directive), fanout);
    assert_eq!(sent_fanout(other.directive), fanout);
    let cli_target_fanout = sent_fanout(cli_target.directive);
    let cli_other_fanout = sent_fanout(cli_other.directive);
    assert_ne!(cli_target_fanout, fanout);
    assert_ne!(cli_other_fanout, fanout);
    assert_ne!(cli_target_fanout, cli_other_fanout);
    for directive in [
        target.directive,
        other.directive,
        cli_target.directive,
        cli_other.directive,
    ] {
        let head = journal
            .read(|read| read.steering_head(directive))
            .expect("steering head reads")
            .expect("steering head exists");
        assert_eq!(head.body().fanout, Some(sent_fanout(directive)));
    }
}

fn assert_fanout_mining(
    fixture: &Fixture,
    target: &wire::intent::DirectiveAnswer,
    other: &wire::intent::DirectiveAnswer,
    cli_target: &wire::intent::DirectiveAnswer,
    cli_other: &wire::intent::DirectiveAnswer,
) {
    let mined = invoke(fixture, &["taste", "mine"], true);
    assert!(mined.status.success(), "mining failed: {:?}", mined.stderr);
    let Outcome::Reply(report): Outcome<serde_json::Value> = json_outcome(&mined) else {
        panic!("mining did not reply: {:?}", mined.stderr)
    };
    assert_eq!(report.body["threshold"], 3);
    assert_eq!(report.body["scanned"], 4);
    assert_eq!(report.body["excluded"], 0);
    assert_eq!(report.body["qualifying"], 1);
    assert_eq!(report.body["findings"][0]["recurrence"], 3);
    assert_eq!(report.body["findings"][0]["standing"], "filed");

    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let final_facts = journal.fact_snapshot().expect("mined facts read");
    let proposals = final_facts
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::TaskAdded>())
        .filter(|added| added.project.starts_with(".maxim_proposal.taste."))
        .collect::<Vec<_>>();
    assert_eq!(proposals.len(), 1);
    let description = proposals[0]
        .description
        .as_deref()
        .expect("mined proposal describes its evidence");
    assert!(description.contains("Recurrence count: 3"), "{description}");
    assert!(description.contains(&target.key), "{description}");
    assert!(!description.contains(&other.key), "{description}");
    assert!(description.contains(&cli_target.key), "{description}");
    assert!(description.contains(&cli_other.key), "{description}");
}

#[test]
fn mining_deduplicates_one_real_canvas_fanout_across_distinct_lane_tapes() {
    const CORRECTION: &str = "Do not use grep for repository searches; use rg instead.";

    let (fixture, resident, lanes) = setup();
    let tracked_before =
        fs::read_to_string(fixture.root.join("spice-rust.toml")).expect("tracked policy reads");
    let (fanout, target, other) = shared_canvas_fanout(&fixture, lanes, CORRECTION);
    let (cli_target, cli_other) = distinct_cli_fanouts(&fixture, lanes, CORRECTION);
    assert_fanout_authority(&fixture, fanout, &target, &other, &cli_target, &cli_other);
    assert_fanout_mining(&fixture, &target, &other, &cli_target, &cli_other);
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml"))
            .expect("tracked policy still reads"),
        tracked_before,
        "mining proposes review work without editing tracked policy"
    );
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
    let spoken = same_message_tool_call_cannot_ack(&fixture, lanes.target);
    rendering_failure_path(&fixture, resident, lanes.target, &historical_key);
    let mut expected = vec![acknowledged, declined, spoken];
    expected.extend(raced);
    verify_authority(&fixture, lanes.target, race, &expected);
}
