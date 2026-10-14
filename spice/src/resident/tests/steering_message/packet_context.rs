//! A directive a room publication delivered presents in its recipient's
//! acknowledgement as that packet: its kind, the result an ANS names, and the
//! packet's body without the header its recipient read. Any other directive
//! stays literal, whatever its words spell.

use super::*;

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
    assert_eq!(context.packet, Some(wire::MessageSendKind::Ans));
    assert_eq!(context.result, Some(result.clone()));
    assert_eq!(context.actor, wire::Actor::Operator);
    assert_eq!(
        context.recipient,
        Some(wire::MessageDestination {
            lane: test_uuid(SENDER),
            occupant: test_uuid(SENDER + 1),
        }),
        "an operator answer addresses the occupant acknowledging it"
    );
    assert!(words.contains("ship"), "{words}");
    assert!(!words.contains("ANS"), "{words}");
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
    assert_eq!(context.packet, Some(wire::MessageSendKind::Msg));
    assert_eq!(context.result, None);
    assert!(words.contains("hold the merge"), "{words}");

    let result = asked(&lanes.say(SENDER, &format!("ASK {first}: why?")));
    let (_, context) = lanes.acknowledged(FIRST, result.as_str());
    assert_eq!(context.packet, Some(wire::MessageSendKind::Ask));
    assert_eq!(context.result, None);

    lanes
        .say(FIRST, &format!("ANS {result}: because"))
        .expect("the ANS publishes");
    let (words, context) = lanes.acknowledged(SENDER, &lanes.held(SENDER, "because"));
    assert_eq!(context.packet, Some(wire::MessageSendKind::Ans));
    assert_eq!(context.result, Some(result));
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
    assert_eq!(context.packet, None);
    assert_eq!(context.result, None);
    assert!(words.contains(&spelled), "{words}");
    lanes.finish();
}

/// A quote link authenticates the directive text its recipient read, then
/// presents the packet that opened it: kind, result, body, and addressed
/// lane. A direction that merely spells a header quotes literally.
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
    let reader = lanes.bus.reader();
    let resolver = spice::quote::Resolver::from_tape(&lanes.paths.tape, &reader);

    let answer = resolver.present(&directive("ship"));
    let [
        wire::QuotePresentation::Resolved {
            text,
            packet,
            result: answered,
            addressee,
            ..
        },
    ] = answer.as_slice()
    else {
        panic!("the answer quote resolves: {answer:?}");
    };
    assert_eq!(*packet, Some(wire::MessageSendKind::Ans));
    assert_eq!(answered.as_ref(), Some(&result));
    assert_eq!(text, "ship");
    assert_eq!(*addressee, Some(test_uuid(SENDER)));

    let literal = resolver.present(&directive("forged"));
    let [
        wire::QuotePresentation::Resolved {
            text,
            packet: None,
            result: None,
            addressee: None,
            ..
        },
    ] = literal.as_slice()
    else {
        panic!("the spelled header quotes as a plain direction: {literal:?}");
    };
    assert_eq!(text, &spelled);
    drop(resolver);
    drop(reader);
    lanes.finish();
}
