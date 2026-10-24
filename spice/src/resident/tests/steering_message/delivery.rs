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
            Err(
                steering::Unpublished::AuthorDirected { reason, .. }
                | steering::Unpublished::Refused { reason, .. },
            ) => reason.as_str(),
            other => panic!("the packet refuses whole: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(reasons, ["msg::recipient-self", "msg::recipient-repeated"]);
    assert_eq!(delivered(&lanes, FIRST).text, "departed");
    assert_eq!(lanes.messages().len(), 1);
    assert_eq!(
        lanes.pending(SENDER).len(),
        1,
        "only the semantic self-address refusal wakes its author"
    );

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
    assert_eq!(lanes.pending(FIRST), std::slice::from_ref(&pending));

    lanes.renew(FIRST, test_uuid(40));
    assert_eq!(lanes.pending(FIRST), std::slice::from_ref(&pending));
    lanes.declare(FIRST);
    assert_eq!(lanes.pending(FIRST), std::slice::from_ref(&pending));

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
        routes: direct(&[first, second]),
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

/// Lane `SECOND` vacated and reseated by a proved successor: the fixture, the
/// expired first address, the successor's address, and both proved births.
struct Reseated {
    lanes: Lanes,
    expired: OccupantAddress,
    current: OccupantAddress,
    renewed: CanonicalUuid,
    expired_birth: bus::OccupantBirth,
    successor_birth: bus::OccupantBirth,
}

impl Reseated {
    fn new(tag: &str) -> Self {
        let lanes = Lanes::new(tag);
        let expired = lanes.address(SECOND);
        lanes.vacate(SECOND);
        let renewed = test_uuid(40);
        lanes.renew(SECOND, renewed);
        let current = lanes
            .bus
            .task_snapshot()
            .expect("snapshot reads")
            .facts()
            .occupant_address(test_uuid(SECOND), renewed)
            .expect("the renewed occupant has one birth");
        assert_ne!(current, expired);
        let birth = |address| {
            lanes
                .bus
                .occupant_birth(address)
                .expect("births read")
                .expect("the address proves one birth")
        };
        let (expired_birth, successor_birth) = (birth(expired), birth(current));
        assert_eq!(expired_birth.lane, successor_birth.lane);
        Self {
            lanes,
            expired,
            current,
            renewed,
            expired_birth,
            successor_birth,
        }
    }

    /// Publish one packet from `SENDER` to `addresses` at transcript `offset`.
    fn publish(
        &self,
        offset: u64,
        addresses: &[OccupantAddress],
        body: &'static str,
    ) -> Result<steering::Published, steering::Unpublished> {
        let packet = steering::Packet {
            lane: test_uuid(SENDER),
            author: test_uuid(SENDER + 1),
            locus: wire::body::TranscriptLocus {
                source: Some(test_uuid(30)),
                member: 1,
                offset,
                length: 10,
                event: 0,
                line: 1,
                column: 1,
            },
            routes: direct(addresses),
            body,
            text: 0..body.len(),
        };
        let view = self.lanes.bus.task_snapshot().expect("snapshot reads");
        steering::publish_packet(
            &self.lanes.bus,
            self.lanes.epoch,
            &steer::Config::default(),
            &packet,
            &view,
        )
    }

    fn interval(&self) -> String {
        steer::inbox::compact_age(self.expired_birth.at, self.successor_birth.at)
    }
}

/// An expired address whose lane now seats a proved successor reaches that
/// successor: its directive records the named address, nothing is withheld,
/// the author hears which route moved, and a reread publishes nothing more. A
/// still-valid address whose lane is vacant queues instead of following any
/// future occupant.
#[test]
fn an_expired_address_reaches_the_proved_successor_its_lane_seats_now() {
    let fixture = Reseated::new("msg-renewed");
    let Reseated {
        lanes,
        expired,
        current,
        renewed,
        ..
    } = &fixture;
    let first = lanes.address(FIRST);
    lanes.vacate(FIRST);

    let resolved = fixture.publish(0, &[first, *expired], "renewed");
    assert_eq!(
        resolved,
        Ok(steering::Published::Sent {
            lanes: vec![test_uuid(SECOND)],
            queued: vec![first],
            redirected: vec![format!(
                "{expired} expired; its lane now seats {current}, born {} after it",
                fixture.interval()
            )],
            unreached: Vec::new(),
            rooms: Vec::new(),
            operator: false,
            held_room: false,
        })
    );
    assert!(
        lanes.withheld().is_empty(),
        "a redirected route withholds nothing"
    );
    let [(_, message)] = lanes.messages().try_into().expect("one message delivers");
    assert_eq!(message.occupant, *renewed);
    assert_eq!(message.text, "renewed");
    assert_eq!(message.addressed, Some(*expired));

    let again = fixture.publish(0, &[first, *expired], "renewed");
    assert_eq!(again, Ok(steering::Published::Already));
    assert_eq!(lanes.messages().len(), 1, "a reread publishes nothing more");
    fixture.lanes.finish();
}

/// One packet naming an earlier code and the current code of one seat opens
/// nothing: the two routes repeat one recipient, whatever contexts they carry.
#[test]
fn two_codes_for_one_seat_repeat_the_recipient() {
    let fixture = Reseated::new("msg-renewed-repeated");
    let Reseated {
        lanes,
        expired,
        current,
        ..
    } = &fixture;
    let resolved = fixture.publish(0, &[*expired, *current], "twice");
    assert_eq!(
        resolved,
        Err(steering::Unpublished::Refused {
            reason: "msg::recipient-repeated".to_owned(),
            detail: format!("{expired} and {current} both name the seat {current} holds"),
        })
    );
    assert!(lanes.messages().is_empty());
    fixture.lanes.finish();
}

/// A lane's new occupant that names its own lane's earlier code reaches
/// itself under no code: the packet refuses as a self recipient and no message
/// from the lane to itself is written.
#[test]
fn an_earlier_code_of_the_authors_own_lane_is_a_self_recipient() {
    let lanes = Lanes::new("msg-self-earlier");
    let earlier = lanes.address(SENDER);
    lanes.vacate(SENDER);
    let renewed = test_uuid(42);
    lanes.renew(SENDER, renewed);
    let packet = steering::Packet {
        lane: test_uuid(SENDER),
        author: renewed,
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 0,
            length: 10,
            event: 0,
            line: 1,
            column: 1,
        },
        routes: direct(&[earlier]),
        body: "to myself",
        text: 0..9,
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
        Err(steering::Unpublished::Refused {
            reason: "msg::recipient-self".to_owned(),
            detail: format!("{earlier} is an earlier address of the authoring occupant's own lane"),
        })
    );
    assert!(lanes.messages().is_empty());
    lanes.finish();
}

/// The successor's inbox projects the named earlier address with both births
/// and renders it in the redirected directive's header; a directly addressed
/// directive on the same lane renders its ordinary header.
#[test]
fn a_redirected_message_tells_its_receiver_whom_it_named() {
    let fixture = Reseated::new("msg-renewed-receiver");
    let Reseated {
        lanes,
        expired,
        current,
        ..
    } = &fixture;
    fixture
        .publish(0, &[*expired], "renewed")
        .expect("the earlier code redirects");
    fixture
        .publish(11, &[*current], "direct")
        .expect("the current code delivers");

    let pending = lanes.pending(SECOND);
    let projected = pending
        .iter()
        .map(|directive| directive.addressed)
        .collect::<Vec<_>>();
    assert_eq!(
        projected,
        [
            Some(wire::intent::PeerAddressed {
                address: *expired,
                born: fixture.expired_birth.at,
                successor_born: fixture.successor_birth.at,
            }),
            None,
        ],
        "only the redirected directive names the earlier occupant"
    );
    let rendered = steer::inbox::render_pending(
        &"1kQnrzNZ".parse().expect("fixture seal"),
        &pending,
        fixture.successor_birth.at,
        |_, _| None,
    );
    assert!(
        rendered.contains(&format!(
            " addressed {expired}] Sent to this lane's earlier occupant {expired}, born {} \
             before you;",
            fixture.interval()
        )),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("[PEER sender").count(),
        2,
        "the directly addressed route renders its ordinary header: {rendered}"
    );
    fixture.lanes.finish();
}

/// Once the successor departs too, the older address stays expired and names
/// that successor; only the latest address queues against the vacant lane.
#[test]
fn a_successor_departure_does_not_revive_an_older_address() {
    let fixture = Reseated::new("msg-renewed-departed");
    let Reseated {
        lanes,
        expired,
        current,
        ..
    } = &fixture;
    lanes.vacate(SECOND);

    let resolved = fixture.publish(11, &[*expired, *current], "departed successor");
    assert_eq!(
        resolved,
        Ok(sent_queued(
            &[],
            &[*current],
            &[&format!(
                "{expired} address::expired (its lane is vacant after {current})"
            )]
        )),
    );
    assert!(
        lanes
            .queued_messages()
            .iter()
            .any(|(_, queued)| queued.recipient == *current),
        "only the latest address can queue against the vacant lane"
    );
    fixture.lanes.finish();
}

/// An expired address whose lane now seats an occupant the journal proves no
/// birth for still receives nothing: the redirect follows only a proved
/// successor, and the author hears the same refusal as before.
#[test]
fn an_expired_address_never_follows_an_unproved_successor() {
    let lanes = Lanes::new("msg-unproved-successor");
    let second = lanes.address(SECOND);
    lanes.vacate(SECOND);
    let unproved = test_uuid(41);
    lanes.seat_unproved(SECOND, unproved);
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
        routes: direct(&[second]),
        body: "to the unproved seat",
        text: 0..20,
    };
    let view = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(
        &lanes.bus,
        lanes.epoch,
        &steer::Config::default(),
        &packet,
        &view,
    );
    assert!(
        matches!(
            &resolved,
            Err(steering::Unpublished::Refused { reason, detail })
                if reason == "msg::recipient-unavailable"
                    && detail.contains(&format!("{second} address::expired"))
        ),
        "{resolved:?}"
    );
    assert!(lanes.messages().is_empty());
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
            Some(Err(steering::Unpublished::AuthorDirected { reason, .. }))
                if reason == "msg::sender-unproved"
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

/// Partial delivery and grammar refusal retain the background-only path.
#[test]
fn partial_and_grammar_refusals_open_no_author_directive() {
    let lanes = Lanes::new("msg-partial");
    let first = lanes.address(FIRST);
    let unrecorded = OccupantAddress::from(wire::Seq::new(9_999_999).expect("nonzero position"));

    lanes.author(
        SENDER,
        &format!("MSG {first} {unrecorded}: partly delivered"),
    );
    let (_, partial) = lanes.hear_newest(SENDER);
    assert!(matches!(
        partial.as_slice(),
        [Ok(steering::Published::Sent { unreached, .. })] if unreached.len() == 1
    ));
    lanes.author(SENDER, &format!("MSG {first}:"));
    let (_, malformed) = lanes.hear_newest(SENDER);
    assert!(matches!(
        malformed.as_slice(),
        [Err(steering::Unpublished::Refused { .. })]
    ));
    let facts = lanes.bus.fact_snapshot().expect("facts read");
    assert!(
        facts
            .facts()
            .iter()
            .all(|envelope| envelope.fact.read::<wire::body::SteerWithheld>().is_none())
    );
    assert!(facts.facts().iter().all(|envelope| {
        envelope
            .fact
            .read::<wire::body::SteerSent>()
            .and_then(|sent| sent.control)
            .and_then(|control| wire::parse_packet_withheld_control(&control))
            .is_none()
    }));
    lanes.finish();
}

/// A fully withheld packet atomically wakes its author once, while a partial
/// miss and grammar refusal retain their non-waking paths.
#[test]
fn a_withheld_packet_wakes_its_author_once_and_rereads_nothing() {
    let lanes = Lanes::new("msg-withheld");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));

    // Each seating writes the same facts, so the next birth lands as far past
    // the cursor the seating starts from as the last one did.
    let stride = second.birth().get() - first.birth().get();
    let lag = lanes.cursor() - second.birth().get();
    let unborn = OccupantAddress::from(
        wire::Seq::new(lanes.cursor() + 1 + stride - lag).expect("nonzero position"),
    );
    lanes.author(SENDER, &format!("MSG {unborn}: not yet born"));

    let (_, outcomes) = lanes.hear_newest(SENDER);
    assert!(
        matches!(
            outcomes.as_slice(),
            [Err(steering::Unpublished::AuthorDirected { reason, .. })]
                if reason == "msg::recipient-unavailable"
        ),
        "{outcomes:?}"
    );
    let snapshot = lanes.bus.fact_snapshot().expect("facts read");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|envelope| envelope.fact.read::<wire::body::SteerWithheld>().is_some())
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|envelope| {
                envelope
                    .fact
                    .read::<wire::body::SteerSent>()
                    .and_then(|body| body.control)
                    .and_then(|control| wire::parse_packet_withheld_control(&control))
                    .is_some()
            })
            .count(),
        1
    );
    let withheld = snapshot
        .facts()
        .iter()
        .find_map(|envelope| {
            envelope
                .fact
                .read::<wire::body::SteerWithheld>()
                .map(|body| (envelope, body))
        })
        .expect("the refusal is durable");
    let directed = snapshot
        .facts()
        .iter()
        .find_map(|envelope| {
            let body = envelope.fact.read::<wire::body::SteerSent>()?;
            wire::parse_packet_withheld_control(body.control.as_deref()?)
                .map(|locus| (envelope, body, locus))
        })
        .expect("the author directive is durable");
    assert_eq!(directed.0.actor, Actor::Spice);
    assert_eq!(directed.0.cause, Some(withheld.0.seq));
    assert_eq!(directed.2, withheld.1.locus);
    assert!(directed.1.text.contains(&format!("MSG {unborn}")));
    assert!(directed.1.text.contains("msg::recipient-unavailable"));
    assert!(directed.1.text.contains(&unborn.to_string()));
    assert_eq!(lanes.pending(SENDER).len(), 1);

    let (_, reread) = lanes.hear_newest(SENDER);
    assert_eq!(reread, [Err(steering::Unpublished::Withheld)]);
    let after = lanes.bus.fact_snapshot().expect("facts read after reread");
    assert_eq!(
        after.horizon(),
        snapshot.horizon(),
        "the reread writes nothing"
    );

    lanes.recover();
    lanes.recover();
    let recovered = lanes
        .bus
        .fact_snapshot()
        .expect("facts read after recovery rereads");
    assert_eq!(
        recovered.horizon(),
        after.horizon(),
        "startup rereads write nothing"
    );
    assert_eq!(lanes.pending(SENDER).len(), 1);
    assert!(
        lanes
            .messages()
            .iter()
            .all(|(_, body)| body.text != "not yet born")
    );
    lanes.finish();
}
