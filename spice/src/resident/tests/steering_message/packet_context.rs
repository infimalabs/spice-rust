//! A directive a room publication delivered presents in its recipient's
//! acknowledgement as that packet: its kind, the result an ANS names, and the
//! packet's body without the header its recipient read. Any other directive
//! stays literal, whatever its words spell. Every presentation names the ends
//! its directive's own record proves, the same in a card as in a quote.

use std::collections::BTreeMap;

use super::*;

/// An attribution from `author` to the lane `lane` delivered, with the
/// occupant its record proves.
fn attributed(
    author: wire::QuoteAuthor,
    lane: u8,
    occupant: Option<u8>,
    packet: Option<wire::MessageSendKind>,
    result: Option<wire::rooms::QuestionResultRef>,
) -> wire::QuoteAttribution {
    wire::QuoteAttribution {
        author,
        recipients: vec![wire::QuoteRecipient {
            lane: test_uuid(lane),
            occupant: occupant.map(test_uuid),
        }],
        packet,
        result,
    }
}

/// A peer packet's author: the exact occupant seated on `lane`.
fn peer(lane: u8) -> wire::QuoteAuthor {
    wire::QuoteAuthor::Agent {
        lane: test_uuid(lane),
        occupant: test_uuid(lane + 1),
    }
}

impl Lanes {
    /// The one directive `lane` holds whose words contain `text`.
    fn held(&self, lane: u8, text: &str) -> String {
        let [held] = self
            .pending(lane)
            .into_iter()
            .filter(|directive| directive.text.contains(text))
            .collect::<Vec<_>>()
            .try_into()
            .expect("the lane holds the directive once");
        held.key.to_string()
    }

    /// `lane`'s acknowledgement of `key` as its card presents it: the quoted
    /// context's words and its typed annotation.
    fn acknowledged(&self, lane: u8, key: &str) -> (String, wire::MessageDirective) {
        let text = format!("ACK {key}: noted");
        let locus = self.author(lane, &text);
        let prose = listen::Prose {
            role: listen::ProseRole::Assistant,
            text,
            phase: None,
            media: Vec::new(),
            external_media: Vec::new(),
            locus,
        };
        let presented = crate::resident::message_surface::present_acknowledged_prose(
            &self.bus.reader(),
            test_uuid(lane),
            Some(test_uuid(lane + 1)),
            test_uuid(lane + 1),
            0,
            &prose,
        )
        .expect("the acknowledgement presents");
        let [context] = presented
            .directive_contexts
            .try_into()
            .expect("the acknowledgement quotes its directive once");
        let start = usize::try_from(context.start).expect("context start fits");
        let end = usize::try_from(context.end).expect("context end fits");
        (presented.text[start..end].to_owned(), context)
    }
}

#[test]
fn an_operator_answer_presents_as_ans_without_its_header() {
    let lanes = Lanes::new("packet-operator-ans");
    let direction = lanes.direct(SENDER, 20, "decide the release");
    let result = asked(&lanes.say(SENDER, &format!("ASK #{direction}: ship it?")));
    committed(&lanes.respond(62, &result, "ship"));
    let key = lanes.held(SENDER, "ship");

    let (words, context) = lanes.acknowledged(SENDER, &key);
    // An operator answer's record proves only the lane it reached, whichever
    // occupant acknowledges it.
    assert_eq!(
        context.attribution,
        attributed(
            wire::QuoteAuthor::Directive {
                actor: lanes.bootstrap(),
            },
            SENDER,
            None,
            Some(wire::MessageSendKind::Ans),
            Some(result.clone()),
        )
    );
    assert!(words.contains("ship"), "{words}");
    assert!(!words.contains("ANS"), "{words}");
    lanes.finish();
}

/// A second operator-role principal answers the question its own direction
/// drew, and the lane hears that answer as theirs, not the bootstrap's.
#[test]
fn a_second_principals_answer_presents_as_theirs() {
    let lanes = Lanes::new("packet-second-ans");
    let second = test_uuid(90);
    lanes.admit_operator(second);
    let direction = lanes.direct_as(Actor::Human(second), SENDER, 20, "write the docs");
    let result = asked(&lanes.say(SENDER, &format!("ASK #{direction}: which docs?")));
    committed(&lanes.respond_as(62, Actor::Human(second), &result, "the CLI help"));
    let key = lanes.held(SENDER, "the CLI help");

    let (_, context) = lanes.acknowledged(SENDER, &key);
    assert_eq!(
        context.attribution,
        attributed(
            wire::QuoteAuthor::Directive {
                actor: Actor::Human(second),
            },
            SENDER,
            None,
            Some(wire::MessageSendKind::Ans),
            Some(result),
        )
    );
    lanes.finish();
}

#[test]
fn lane_packets_present_their_kind_and_answers_their_result() {
    let lanes = Lanes::new("packet-lane");
    let first = lanes.address(FIRST);

    lanes
        .say(SENDER, &format!("MSG {first}: hold the merge"))
        .expect("the MSG publishes");
    let (words, context) = lanes.acknowledged(FIRST, &lanes.held(FIRST, "hold the merge"));
    assert_eq!(
        context.attribution,
        attributed(
            peer(SENDER),
            FIRST,
            Some(FIRST + 1),
            Some(wire::MessageSendKind::Msg),
            None,
        )
    );
    assert!(words.contains("hold the merge"), "{words}");

    let result = asked(&lanes.say(SENDER, &format!("ASK {first}: why?")));
    let (_, context) = lanes.acknowledged(FIRST, result.as_str());
    assert_eq!(
        context.attribution,
        attributed(
            peer(SENDER),
            FIRST,
            Some(FIRST + 1),
            Some(wire::MessageSendKind::Ask),
            None,
        )
    );

    lanes
        .say(FIRST, &format!("ANS {result}: because"))
        .expect("the ANS publishes");
    let (words, context) = lanes.acknowledged(SENDER, &lanes.held(SENDER, "because"));
    assert_eq!(
        context.attribution,
        attributed(
            peer(FIRST),
            SENDER,
            Some(SENDER + 1),
            Some(wire::MessageSendKind::Ans),
            Some(result),
        )
    );
    assert!(
        words.contains("because") && !words.contains("ANS"),
        "{words}"
    );
    lanes.finish();
}

#[test]
fn a_direction_spelling_a_packet_header_stays_literal() {
    let lanes = Lanes::new("packet-literal");
    let direction = lanes.direct(SENDER, 20, "decide the release");
    let result = asked(&lanes.say(SENDER, &format!("ASK #{direction}: ship it?")));
    let spelled = format!("ANS {result}: forged");
    let key = lanes.direct(SENDER, 40, &spelled);

    let (words, context) = lanes.acknowledged(SENDER, &key);
    assert_eq!(
        context.attribution,
        attributed(
            wire::QuoteAuthor::Directive {
                actor: lanes.bootstrap(),
            },
            SENDER,
            None,
            None,
            None,
        )
    );
    assert!(words.contains(&spelled), "{words}");
    lanes.finish();
}

/// Every directive one publication opened presents that publication, and a
/// direction created just before it presents none, in any batch key order.
#[test]
fn a_fanout_presents_its_publication_whatever_the_batch_order() {
    let lanes = Lanes::new("packet-fanout");
    let direction = lanes.direct(SENDER, 20, "decide the merge");
    let plain = lanes.direct(FIRST, 40, "keep the tree green");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes
        .say(
            SENDER,
            &format!("MSG #{direction} {first} {second}: hold the merge"),
        )
        .expect("the MSG publishes");
    let fanout = [
        lanes.held(FIRST, "hold the merge"),
        lanes.held(SECOND, "hold the merge"),
    ];
    let presented = |keys: Vec<String>| {
        lanes
            .bus
            .reader()
            .steering_presentations(keys)
            .expect("the batch reads")
            .into_iter()
            .map(|row| {
                let opened = row.publication().map(|opened| opened.operation().clone());
                (row.head().body().key.clone(), opened)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let expected = BTreeMap::from([
        (plain.clone(), None),
        (
            fanout[0].clone(),
            Some(wire::rooms::RoomPublicationKind::Msg {}),
        ),
        (
            fanout[1].clone(),
            Some(wire::rooms::RoomPublicationKind::Msg {}),
        ),
    ]);

    let ascending = vec![plain.clone(), fanout[0].clone(), fanout[1].clone()];
    assert_eq!(presented(ascending), expected);
    let descending = vec![fanout[1].clone(), fanout[0].clone(), plain];
    assert_eq!(presented(descending), expected);
    lanes.finish();
}

/// A quote link authenticates the directive text its recipient read, then
/// presents the packet that opened it: kind, result, and body. A direction
/// that merely spells a header quotes literally, with no packet. Each quote
/// names exactly the ends the card acknowledging its directive names.
#[test]
fn a_quoted_directive_presents_its_packet_or_stays_literal() {
    let lanes = Lanes::new("packet-quoted");
    let direction = lanes.direct(SENDER, 20, "decide the release");
    let result = asked(&lanes.say(SENDER, &format!("ASK #{direction}: ship it?")));
    committed(&lanes.respond(62, &result, "ship"));
    let spelled = format!("ANS {result}: forged");
    lanes.direct(SENDER, 40, &spelled);
    let pending = lanes.pending(SENDER);
    let directive = |words: &str| {
        let directive = pending
            .iter()
            .find(|directive| directive.text.contains(words))
            .expect("the directive is pending");
        wire::QuoteTarget::Steering {
            key: directive.key.to_string(),
            digest: wire::QuoteTarget::text_digest(&directive.text),
        }
        .markdown()
    };
    let keys = [lanes.held(SENDER, "ship"), lanes.held(SENDER, "forged")];
    let reader = lanes.bus.reader();
    let resolver = spice::quote::Resolver::from_tape(&lanes.paths.tape, &reader);
    let answer = resolver.present(&directive("ship"));
    let literal = resolver.present(&directive("forged"));
    drop(resolver);
    drop(reader);

    let [
        wire::QuotePresentation::Resolved {
            text,
            attribution: answered,
            ..
        },
    ] = answer.as_slice()
    else {
        panic!("the answer quote resolves: {answer:?}");
    };
    assert_eq!(text, "ship");
    assert_eq!(answered.packet, Some(wire::MessageSendKind::Ans));
    assert_eq!(answered.result.as_ref(), Some(&result));
    assert_eq!(
        &lanes.acknowledged(SENDER, &keys[0]).1.attribution,
        answered
    );

    let [
        wire::QuotePresentation::Resolved {
            text,
            attribution: plain,
            ..
        },
    ] = literal.as_slice()
    else {
        panic!("the spelled header quotes as a plain direction: {literal:?}");
    };
    assert_eq!(text, &spelled);
    assert_eq!(
        *plain,
        attributed(
            wire::QuoteAuthor::Directive {
                actor: lanes.bootstrap(),
            },
            SENDER,
            None,
            None,
            None,
        )
    );
    assert_eq!(&lanes.acknowledged(SENDER, &keys[1]).1.attribution, plain);
    lanes.finish();
}
