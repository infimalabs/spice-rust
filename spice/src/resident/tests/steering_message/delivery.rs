//! A packet reaches each seated recipient, queues against a proved vacant
//! lane, names every address it did not reach, and is judged once against the
//! residency it resolved: a later seat change neither revives nor repeats it.

use super::*;

/// Each seated recipient hears the packet, a proved vacant lane queues it,
/// and every other unavailable address is named with why. A packet naming its
/// own author or an address twice still publishes nothing.
#[test]
fn a_packet_reaches_each_seated_recipient_and_names_each_unavailable_address() {
    let lanes = Lanes::new("msg-refuse");
    let (sender, first, second) = (
        lanes.address(SENDER),
        lanes.address(FIRST),
        lanes.address(SECOND),
    );
    lanes.vacate(SECOND);
    let unrecorded = OccupantAddress::from(wire::Seq::new(9_999_999).expect("nonzero position"));
    lanes.author(SENDER, &format!("MSG {first} {second}: departed"));
    lanes.author(SENDER, &format!("MSG {second} {unrecorded}: nobody"));
    lanes.author(SENDER, &format!("MSG {first} {sender}: myself"));
    lanes.author(SENDER, &format!("MSG {first} {first}: twice"));

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        outcomes[..2],
        [
            Ok(sent_queued(&[FIRST], &[second], &[])),
            Ok(sent_queued(
                &[],
                &[second],
                &[&format!("{unrecorded} address::unrecorded")]
            )),
        ],
        "{diagnostics}"
    );
    let reasons = outcomes[2..]
        .iter()
        .map(|outcome| match outcome {
            Err(steering::Unpublished::Refused { reason, .. }) => reason.as_str(),
            other => panic!("the packet refuses whole: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(reasons, ["msg::recipient-self", "msg::recipient-repeated"]);
    assert_eq!(delivered(&lanes, FIRST).text, "departed");
    assert_eq!(lanes.messages().len(), 1);

    let (reread, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        reread[0],
        Ok(steering::Published::Already),
        "a partly delivered packet is published"
    );
    assert_eq!(lanes.messages().len(), 1, "a reread sends nothing");
    lanes.finish();
}

/// A valid historical address binds one queued directive to its vacant
/// logical lane. Resident recovery, seating, session declaration, and later
/// renewal expose that same live authority without minting another identity or
/// deadline.
#[test]
fn a_vacant_lane_inherits_one_unchanged_peer_message_at_each_later_seat() {
    let lanes = Lanes::new("msg-queued");
    let first = lanes.address(FIRST);
    lanes.vacate(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: wait for the next occupant"));

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        outcomes,
        [Ok(sent_queued(&[], &[first], &[]))],
        "{diagnostics}"
    );
    let queued_messages = lanes.queued_messages();
    let [(_, queued)] = queued_messages.as_slice() else {
        panic!("the vacant route opens exactly one queued fact")
    };
    assert_eq!(queued.recipient, first);
    assert_eq!(queued.lane, test_uuid(FIRST));
    assert!(
        lanes.messages().is_empty(),
        "no seated delivery was fabricated"
    );

    let pending = delivered(&lanes, FIRST);
    lanes.restart();
    assert_eq!(lanes.pending(FIRST), [pending.clone()]);

    lanes.renew(FIRST, test_uuid(40));
    assert_eq!(lanes.pending(FIRST), [pending.clone()]);
    lanes.declare(FIRST);
    assert_eq!(lanes.pending(FIRST), [pending.clone()]);

    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(41));
    assert_eq!(lanes.pending(FIRST), [pending]);
    assert_eq!(
        lanes.queued_messages().len(),
        1,
        "recovery and seat changes mint no replacement"
    );
    lanes.finish();
}

/// The commit re-proves the residency the packet resolved against, so a seat
/// change in between refuses everything; the next resolution judges the
/// residency as it now stands.
#[test]
fn a_residency_move_between_resolution_and_commit_publishes_to_nobody() {
    let lanes = Lanes::new("msg-race");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    let stale = lanes.bus.task_snapshot().expect("snapshot reads");
    lanes.vacate(SECOND);
    let packet = steering::Packet {
        lane: test_uuid(SENDER),
        author: test_uuid(SENDER + 1),
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 0,
            length: 10,
            event: 0,
            line: 1,
            column: 1,
        },
        request: steer::ack::scan::Request::Msg {
            routes: direct(&[first, second]),
        },
        body: "raced",
        text: 0..5,
    };
    let config = steer::Config::default();

    let raced = steering::publish_packet(&lanes.bus, lanes.epoch, &config, &packet, &stale);
    assert!(
        matches!(
            &raced,
            Err(steering::Unpublished::Refused { reason, .. }) if reason == "steer::residency-moved"
        ),
        "{raced:?}"
    );
    assert!(lanes.messages().is_empty());

    let current = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(&lanes.bus, lanes.epoch, &config, &packet, &current);
    assert_eq!(resolved, Ok(sent_queued(&[FIRST], &[second], &[])));
    assert_eq!(lanes.messages().len(), 1);
    lanes.finish();
}

/// An expired address receives nothing, and its author learns what its birth
/// lane seats in the view the packet resolved against. A still-valid address
/// whose lane is vacant queues instead of following any future occupant.
#[test]
fn an_expired_address_names_what_its_lane_seats_now() {
    let lanes = Lanes::new("msg-renewed");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.vacate(SECOND);
    lanes.vacate(FIRST);
    let renewed = test_uuid(40);
    lanes.renew(SECOND, renewed);
    let current = lanes
        .bus
        .task_snapshot()
        .expect("snapshot reads")
        .facts()
        .occupant_address(test_uuid(SECOND), renewed)
        .expect("the renewed occupant has one birth");
    assert_ne!(current, second);
    let packet = steering::Packet {
        lane: test_uuid(SENDER),
        author: test_uuid(SENDER + 1),
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 0,
            length: 10,
            event: 0,
            line: 1,
            column: 1,
        },
        request: steer::ack::scan::Request::Msg {
            routes: direct(&[first, second, current]),
        },
        body: "renewed",
        text: 0..7,
    };
    let view = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(
        &lanes.bus,
        lanes.epoch,
        &steer::Config::default(),
        &packet,
        &view,
    );

    assert_eq!(
        resolved,
        Ok(sent_queued(
            &[SECOND],
            &[first],
            &[&format!(
                "{second} address::expired (its lane seats {current})"
            )]
        ))
    );
    let messages = lanes.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].1.occupant, renewed,
        "only the address naming the renewed occupant reaches it"
    );

    lanes.vacate(SECOND);
    let after_departure = steering::Packet {
        lane: test_uuid(SENDER),
        author: test_uuid(SENDER + 1),
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 11,
            length: 19,
            event: 1,
            line: 2,
            column: 1,
        },
        request: steer::ack::scan::Request::Msg {
            routes: direct(&[second, current]),
        },
        body: "departed successor",
        text: 0..19,
    };
    let view = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(
        &lanes.bus,
        lanes.epoch,
        &steer::Config::default(),
        &after_departure,
        &view,
    );
    assert_eq!(
        resolved,
        Ok(sent_queued(
            &[],
            &[current],
            &[&format!(
                "{second} address::expired (its lane is vacant after {current})"
            )]
        )),
        "a successor's departure does not revive an older address"
    );
    assert!(
        lanes
            .queued_messages()
            .iter()
            .any(|(_, queued)| queued.recipient == current),
        "only the latest address can queue against the vacant lane"
    );
    lanes.finish();
}

/// Sender identity is the record's capture binding, and a packet answers
/// nothing: the author's own pending direction stays live.
#[test]
fn a_packet_is_attributed_to_its_author_and_retires_nothing() {
    let lanes = Lanes::new("msg-author");
    let first = lanes.address(FIRST);
    let direction = lanes.direct(SENDER, 40, "keep going");
    lanes.author(SENDER, &format!("MSG {first}: about {direction}"));

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(outcomes, [Ok(sent(&[FIRST], &[]))], "{diagnostics}");
    let keys = lanes
        .pending(SENDER)
        .into_iter()
        .map(|pending| pending.key)
        .collect::<BTreeSet<_>>();
    assert_eq!(keys, BTreeSet::from([direction]));

    // A record heard under an occupant with no birth on the lane is no one's.
    lanes.author(SENDER, &format!("MSG {first}: impostor"));
    let (outcomes, _) = lanes.hear(SENDER, test_uuid(FIRST + 1));
    assert!(
        matches!(
            outcomes.last(),
            Some(Err(steering::Unpublished::Refused { reason, .. })) if reason == "msg::sender-unproved"
        ),
        "{outcomes:?}"
    );
    assert_eq!(lanes.messages().len(), 1);
    lanes.finish();
}

/// Startup rereads what the lane's declared occupant wrote after its session
/// declaration, publishing what a crash left behind and nothing twice.
#[test]
fn startup_recovery_publishes_only_unpublished_packets_after_the_declaration() {
    let lanes = Lanes::new("msg-recover");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: before the session"));
    let declaration = lanes.declare(SENDER);

    lanes.author(SENDER, &format!("MSG {first}: heard live"));
    let (live, _) = lanes.hear_newest(SENDER);
    assert!(live.offset > declaration.offset);
    lanes.author(SENDER, &format!("MSG {first}: lost to a crash"));

    lanes.recover();
    lanes.recover();
    let texts = lanes
        .messages()
        .into_iter()
        .map(|(_, body)| (body.text, body.sender))
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            ("heard live".to_owned(), test_uuid(SENDER + 1)),
            ("lost to a crash".to_owned(), test_uuid(SENDER + 1)),
        ],
    );
    lanes.finish();
}

/// A refusal is the packet's settled judgment: once the residency refused it,
/// recovery publishes nothing even after the refused address names a seated
/// occupant.
#[test]
fn a_refused_packet_stays_unpublished_after_its_refusal_stops_holding() {
    let lanes = Lanes::new("msg-withheld");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    // Each seating writes the same facts, so the next birth lands as far past
    // the cursor the seating starts from as the last one did.
    let stride = second.birth().get() - first.birth().get();
    let lag = lanes.cursor() - second.birth().get();
    lanes.declare(SENDER);
    // Judging the packet journals its one refusal before the next seating.
    let unborn = OccupantAddress::from(
        wire::Seq::new(lanes.cursor() + 1 + stride - lag).expect("nonzero position"),
    );
    lanes.author(SENDER, &format!("MSG {unborn}: not yet born"));

    let (_, outcomes) = lanes.hear_newest(SENDER);
    assert!(
        matches!(
            outcomes.as_slice(),
            [Err(steering::Unpublished::Refused { reason, .. })] if reason == "msg::recipient-unavailable"
        ),
        "{outcomes:?}"
    );
    lanes.seat(THIRD);
    assert_eq!(
        lanes.address(THIRD),
        unborn,
        "the refused address now seats"
    );

    lanes.recover();
    lanes.recover();
    assert!(lanes.messages().is_empty());
    assert!(lanes.pending(THIRD).is_empty());
    lanes.finish();
}
