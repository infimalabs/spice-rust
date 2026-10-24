//! A directive an MSG delivered presents in its recipient's acknowledgement
//! as that packet: the packet's body without the header its recipient read.
//! Any other directive stays literal, whatever its words spell. Every
//! presentation names the ends its directive's own record proves, the same in
//! a card as in a quote.

use std::collections::BTreeMap;

use super::*;

/// An attribution from `author` to the lane `lane` delivered, with the
/// occupant its record proves.
fn attributed(
    author: wire::QuoteAuthor,
    lane: u8,
    occupant: Option<u8>,
    packet: bool,
) -> wire::QuoteAttribution {
    wire::QuoteAttribution {
        author,
        recipients: vec![wire::QuoteRecipient {
            lane: test_uuid(lane),
            occupant: occupant.map(test_uuid),
        }],
        packet,
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
fn a_lane_msg_presents_as_its_packet() {
    let lanes = Lanes::new("packet-lane");
    let first = lanes.address(FIRST);

    lanes
        .say(SENDER, &format!("MSG {first}: hold the merge"))
        .expect("the MSG publishes");
    let (words, context) = lanes.acknowledged(FIRST, &lanes.held(FIRST, "hold the merge"));
    assert_eq!(
        context.attribution,
        attributed(peer(SENDER), FIRST, Some(FIRST + 1), true)
    );
    assert!(words.contains("hold the merge"), "{words}");
    lanes.finish();
}

#[test]
fn a_direction_spelling_a_packet_header_stays_literal() {
    let lanes = Lanes::new("packet-literal");
    let spelled = format!("MSG {}: forged", lanes.address(FIRST));
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
            false,
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
/// presents the packet that opened it by its body. A direction that merely
/// spells a header quotes literally, with no packet. Each quote names exactly
/// the ends the card acknowledging its directive names.
#[test]
fn a_quoted_directive_presents_its_packet_or_stays_literal() {
    let lanes = Lanes::new("packet-quoted");
    let direction = lanes.direct(SENDER, 20, "decide the release");
    let first = lanes.address(FIRST);
    lanes
        .say(SENDER, &format!("MSG #{direction} {first}: ship"))
        .expect("the MSG publishes");
    let spelled = format!("MSG {}: forged", lanes.address(SENDER));
    lanes.direct(FIRST, 40, &spelled);
    let pending = lanes.pending(FIRST);
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
    let keys = [lanes.held(FIRST, "ship"), lanes.held(FIRST, "forged")];
    let reader = lanes.bus.reader();
    let resolver = spice::quote::Resolver::from_tape(&lanes.paths.tape, &reader);
    let message = resolver.present(&directive("ship"));
    let literal = resolver.present(&directive("forged"));
    drop(reader);

    let [
        wire::QuotePresentation::Resolved {
            text,
            attribution: sent,
            ..
        },
    ] = message.as_slice()
    else {
        panic!("the MSG quote resolves: {message:?}");
    };
    assert_eq!(text, "ship");
    assert!(sent.packet);
    assert_eq!(&lanes.acknowledged(FIRST, &keys[0]).1.attribution, sent);

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
            FIRST,
            None,
            false,
        )
    );
    assert_eq!(&lanes.acknowledged(FIRST, &keys[1]).1.attribution, plain);
    lanes.finish();
}
