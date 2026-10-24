//! Each MSG is a duty of its recipient's logical lane: an answer retires its
//! own key once and relays one reply to the sending lane, through vacancy,
//! renewal, restart, unreadable transcripts, and lane removal.

use super::*;

/// Every fanout recipient owns its key: each answer retires only its own
/// directive and relays its clause once to the sending lane, whose answer to
/// that reply sends nothing back.
#[test]
fn each_recipient_answer_retires_its_own_msg_and_replies_once_to_the_sending_lane() {
    let lanes = Lanes::new("msg-answer");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.author(SENDER, &format!("MSG {first} {second}: rebase onto main"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let (to_first, to_second) = (delivered(&lanes, FIRST), delivered(&lanes, SECOND));
    assert_eq!(to_first.text, "rebase onto main");
    assert_ne!(to_first.key, to_second.key);
    lanes.declare(FIRST);
    lanes.declare(SECOND);

    lanes.answer(
        FIRST,
        &[&to_first.key],
        &format!("ACK {}: Rebased.", to_first.key),
    );
    assert!(lanes.pending(FIRST).is_empty());
    assert_eq!(
        lanes.pending(SECOND),
        std::slice::from_ref(&to_second),
        "a sibling recipient's key stays live"
    );
    let [ack] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("one reply reaches the sender: {pending:?}"));
    assert_eq!(ack.from, Some(test_uuid(FIRST)));
    assert_eq!(ack.text, "Rebased.");
    assert_eq!(
        ack.control.as_deref(),
        Some(wire::peer_reply_control(&to_first.key, wire::PeerAnswer::Ack).as_str())
    );
    assert!(!ack.wakes, "a peer reply never wakes its lane");
    assert_eq!(
        ack.sender,
        Some(wire::intent::PeerSender {
            occupant: test_uuid(FIRST + 1),
            address: Some(first),
        }),
        "a reply names the exact occupant that answered"
    );

    lanes.answer(
        SECOND,
        &[&to_second.key],
        &format!("NAK {}: Main is red.", to_second.key),
    );
    assert!(lanes.pending(SECOND).is_empty());
    let replies = lanes.pending(SENDER);
    let nak = replies
        .iter()
        .find(|reply| reply.from == Some(test_uuid(SECOND)))
        .unwrap_or_else(|| panic!("the refusal replies too: {replies:?}"));
    assert_eq!(nak.text, "Main is red.");
    assert_eq!(
        nak.sender.map(|sender| sender.address),
        Some(Some(second)),
        "each reply names its own answerer"
    );
    assert_eq!(
        nak.control.as_deref(),
        Some(wire::peer_reply_control(&to_second.key, wire::PeerAnswer::Nak).as_str())
    );

    let keys = replies
        .iter()
        .map(|reply| reply.key.as_str())
        .collect::<Vec<_>>();
    lanes.answer(SENDER, &keys, &format!("ACK {}: Thanks.", keys.join(" ")));
    assert!(lanes.pending(SENDER).is_empty());
    assert!(lanes.pending(FIRST).is_empty() && lanes.pending(SECOND).is_empty());
    let replied = lanes
        .bus
        .fact_snapshot()
        .expect("authority reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::body::SteerReplied>())
        .map(|reply| reply.answered)
        .collect::<Vec<_>>();
    assert_eq!(
        replied,
        [to_first.key, to_second.key],
        "one reply per answered MSG, none for an answered reply"
    );
    lanes.finish();
}

/// A reply follows lane continuity: the lane of a sender that has left hears
/// the answer at its next seat, and a restart neither drops nor repeats it.
#[test]
fn a_reply_to_a_departed_sender_waits_on_its_lane_across_a_restart() {
    let lanes = Lanes::new("msg-reply-vacant");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: is the parser settled?"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    lanes.vacate(SENDER);
    let message = delivered(&lanes, FIRST);

    lanes.answer(FIRST, &[&message.key], &format!("ACK {}", message.key));
    assert!(lanes.pending(FIRST).is_empty(), "the MSG retires");
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("the vacant lane holds one reply: {pending:?}"));
    assert_eq!(
        reply.control.as_deref(),
        Some(wire::peer_reply_control(&message.key, wire::PeerAnswer::Ack).as_str())
    );
    assert!(!reply.wakes, "a peer reply never wakes its vacant lane");
    assert_eq!(reply.text, "", "a bare header relays no clause");

    let mut diagnostics = Vec::new();
    steering::dispose_at_startup(&lanes.bus, &lanes.paths, lanes.epoch, &mut diagnostics)
        .expect("the restart sweep runs");
    assert_eq!(
        lanes.pending(SENDER),
        [reply],
        "a restart neither drops nor repeats the waiting reply"
    );
    lanes.finish();
}

/// An MSG is bound to its recipient's logical lane: it keeps its one key,
/// body, and deadline through that lane's vacancy and renewal, the successor
/// answers it, the reply names the occupant that answered, and an answered
/// MSG never returns at a later renewal. Its quote still names the occupant
/// that wrote it after that sender leaves.
#[test]
fn an_msg_follows_its_lane_to_a_renewed_occupant_with_one_key_and_deadline() {
    let lanes = Lanes::new("msg-lane-bound");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    lanes.vacate(SENDER);
    let message = delivered(&lanes, FIRST);
    let resolver_source = lanes.bus.reader();
    let resolver = spice::quote::Resolver::from_tape(&lanes.paths.tape, &resolver_source);
    let target = wire::QuoteTarget::Steering {
        key: message.key.clone(),
        digest: wire::QuoteTarget::text_digest(&message.text),
    };
    assert!(
        matches!(
            resolver.resolve(&target.to_string(), 0),
            wire::quote::QuoteResolution::Resolved(wire::quote::ResolvedQuote {
                author: wire::QuoteAuthor::Agent { lane, occupant },
                ..
            }) if lane == test_uuid(SENDER) && occupant == test_uuid(SENDER + 1)
        ),
        "a quote names the occupant that wrote the MSG"
    );
    drop(resolver_source);

    lanes.vacate(FIRST);
    assert_eq!(
        lanes.pending(FIRST),
        std::slice::from_ref(&message),
        "a vacant lane keeps its MSG"
    );
    let successor = test_uuid(41);
    lanes.renew(FIRST, successor);
    let before = lanes.cursor();
    lanes.dead_letter_removed_lanes();
    assert_eq!(
        lanes.cursor(),
        before,
        "a lane still resident parks nothing"
    );
    lanes.restart();
    lanes.restart();
    assert_eq!(
        lanes.pending(FIRST),
        std::slice::from_ref(&message),
        "the successor inherits the same key, body, and deadline through every replay"
    );
    assert_eq!(lanes.messages().len(), 1, "replay publishes nothing again");

    lanes.declare(FIRST);
    lanes.answer(
        FIRST,
        &[&message.key],
        &format!("ACK {}: Holding.", message.key),
    );
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the successor's answer retires the inherited key"
    );
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("one reply reaches the sending lane: {pending:?}"));
    let answerer = lanes
        .bus
        .task_snapshot()
        .expect("snapshot reads")
        .facts()
        .occupant_address(test_uuid(FIRST), successor)
        .expect("the successor has one birth");
    assert_ne!(answerer, first);
    assert_eq!(
        reply.sender,
        Some(wire::intent::PeerSender {
            occupant: successor,
            address: Some(answerer),
        }),
        "the reply names the occupant that answered, not the one first addressed"
    );

    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(43));
    assert!(
        lanes.pending(FIRST).is_empty(),
        "an answered MSG never returns to the lane"
    );
    lanes.finish();
}

/// A predecessor transcript that cannot be read at exit settles nothing and
/// says so; once readable, the answer it holds retires the key exactly once.
#[test]
fn an_unreadable_predecessor_transcript_settles_nothing_at_exit() {
    let lanes = Lanes::new("msg-unreadable-predecessor");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);
    lanes.declare(FIRST);
    lanes.answer(FIRST, &[], &format!("ACK {}: Holding.", message.key));
    lanes.vacate(FIRST);
    let tape = lanes
        .paths
        .tape
        .join(test_uuid(FIRST).to_string())
        .join("omp");
    let hidden = tape.with_extension("hidden");
    fs::rename(&tape, &hidden).expect("the tape moves aside");
    fs::write(&tape, b"not a tape").expect("an unreadable tape stands in");
    let vendor: wire::Name = "omp".parse().expect("fixture vendor");
    let exit = |diagnostics: &mut Vec<u8>| {
        steering::dispose_at_exit(
            &lanes.bus,
            &lanes.paths,
            lanes.epoch,
            test_uuid(FIRST),
            &vendor,
            diagnostics,
        );
    };

    let mut diagnostics = Vec::new();
    exit(&mut diagnostics);

    assert_eq!(
        lanes.pending(FIRST),
        std::slice::from_ref(&message),
        "an unreadable predecessor settles nothing"
    );
    assert!(lanes.pending(SENDER).is_empty(), "no reply is owed yet");
    assert!(
        String::from_utf8_lossy(&diagnostics).contains(&test_uuid(FIRST).to_string()),
        "the failure is visible"
    );

    fs::remove_file(&tape).expect("the stand-in leaves");
    fs::rename(&hidden, &tape).expect("the tape returns");
    exit(&mut Vec::new());
    exit(&mut Vec::new());
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the readable answer retires"
    );
    assert_eq!(
        lanes.pending(SENDER).len(),
        1,
        "one reply reaches the sender"
    );
    lanes.finish();
}

/// Removing a lane ends its peer duties' destination: each live one parks
/// once with the lane-removed reason, a lane that takes its place inherits
/// nothing, and recovery refuses to revive the duty.
#[test]
fn a_removed_lane_parks_its_msg_and_a_replacement_lane_inherits_nothing() {
    let lanes = Lanes::new("msg-lane-removed");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);

    lanes.vacate(FIRST);
    lanes.remove(FIRST);
    lanes.seat(THIRD);
    let before = lanes.cursor();
    // No launch was attempted for a removed lane, so its one line names the
    // directive and the removal itself as the reason.
    let log = lanes.dead_letter_removed_lanes();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains(&message.key.to_string()), "{log}");
    assert!(log.contains(steering::LANE_REMOVED), "{log}");
    let head = lanes
        .bus
        .read(move |read| read.steering_head(message.directive))
        .expect("steering reads")
        .expect("the MSG stays durable");
    assert_eq!(head.body().disposition, wire::Disposition::DeadLettered);
    assert_eq!(head.body().key, message.key, "parking never re-keys");
    let reasons = lanes
        .bus
        .fact_snapshot()
        .expect("authority reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::steering::SteerDeadLettered>())
        .map(|parked| (parked.directive, parked.reason))
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [(message.directive, steering::LANE_REMOVED.to_owned())]
    );
    let parked = lanes.cursor();
    assert!(parked > before);
    assert_eq!(
        lanes.dead_letter_removed_lanes(),
        "",
        "a parked MSG parks once"
    );
    assert_eq!(lanes.cursor(), parked, "a parked MSG parks once");
    assert!(
        lanes.pending(THIRD).is_empty(),
        "a replacement lane inherits nothing"
    );

    let refused = lanes.recover_directive(message.directive);
    assert!(
        matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == steering::LANE_REMOVED),
        "{refused:?}"
    );
    lanes.finish();
}
