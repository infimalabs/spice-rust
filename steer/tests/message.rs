//! Reading authored MSG packets out of assistant prose.
//!
//! Publication packets share the acknowledgment header grammar's
//! authored-context rules but publish rather than retire: each placed packet
//! names its routes, each an audience, a task context, or an audience in a
//! context, and owns exactly the body hunk in its paragraph.

use steer::ack::keys::Key;
use steer::ack::scan::{
    Audience, Message, MessageRefusal, Outcome, Standing, answer, has_acknowledgement, has_message,
    placed_headers, scan, scan_acknowledgements, scan_messages,
};
use wire::{Disposition, OccupantAddress, OccupantAddressError, TaskHandle};

/// Every key any fixture names is live.
struct Live;

impl Standing for Live {
    fn disposition(&self, _: &Key) -> Option<Disposition> {
        Some(Disposition::Live)
    }
}

fn address(text: &str) -> OccupantAddress {
    text.parse().expect("fixture address")
}

/// Selectors as written and the exact body bytes one packet publishes.
fn sent<'prose>(prose: &'prose str, message: &Message) -> (Vec<&'prose str>, &'prose str) {
    let publication = message
        .publication
        .as_ref()
        .unwrap_or_else(|refusal| panic!("refused {refusal:?} in {prose:?}"));
    let selectors = publication
        .routes
        .iter()
        .map(|route| {
            let written = &prose[route.span.clone()];
            let (audience, context) = written.split_once('#').unwrap_or((written, ""));
            assert_eq!(
                route.audience,
                (!audience.is_empty()).then(|| Audience::Occupant(address(audience)))
            );
            assert_eq!(
                route.context,
                (!context.is_empty()).then(|| handle(context))
            );
            written
        })
        .collect();
    (selectors, &prose[publication.body.clone()])
}

fn handle(text: &str) -> TaskHandle {
    text.parse().expect("fixture handle")
}

fn refusal(prose: &str) -> MessageRefusal {
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 1, "expected one packet in {prose:?}");
    messages[0]
        .publication
        .clone()
        .expect_err("packet is refused")
}

/// Each selector owns its own qualifier: room records, directed-in-room, and
/// direct occupant routes share one body without lending each other context.
#[test]
fn selectors_route_to_occupants_rooms_or_both() {
    let prose = "MSG #ROOMS-dVrD244Jd: Background note.\n\
                 MSG 6HmS#ROOMS-dVrD244Jd 7Bc, #DESIGN-dVrD1Kz1l: Shared.";
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 2);
    assert_eq!(
        sent(prose, &messages[0]),
        (vec!["#ROOMS-dVrD244Jd"], "Background note.")
    );
    assert_eq!(
        sent(prose, &messages[1]),
        (
            vec!["6HmS#ROOMS-dVrD244Jd", "7Bc", "#DESIGN-dVrD1Kz1l"],
            "Shared."
        )
    );
    let publication = messages[1].publication.as_ref().expect("published");
    assert_eq!(
        publication.addresses().collect::<Vec<_>>(),
        [address("6HmS"), address("7Bc")]
    );
}

/// A task pointer records its room and reaches nobody; the context-free legacy
/// operator spelling is not an audience either.
#[test]
fn a_task_pointer_records_the_room_without_reaching_the_operator() {
    let prose = "MSG #ROOMS-dVrD244Jd: Background note.";
    let [message] = scan_messages(prose).try_into().expect("one packet");
    let publication = message.publication.expect("the task pointer records");
    let [route] = publication.routes.as_slice() else {
        panic!("one route");
    };
    assert_eq!(route.audience, None);
    assert_eq!(route.context, Some(handle("ROOMS-dVrD244Jd")));

    assert_eq!(
        refusal("MSG operator: context-free"),
        MessageRefusal::Concern { span: 4..12 }
    );
    assert_eq!(
        refusal("MSG operator#ROOMS-dVrD244Jd: still nowhere"),
        MessageRefusal::Concern { span: 4..12 }
    );
}

/// A malformed qualifier, a repeated destination, or a room named twice
/// refuses the whole packet.
#[test]
fn malformed_or_repeated_destinations_refuse_the_packet() {
    assert!(matches!(
        refusal("MSG 6HmS#rooms-x: lower stem."),
        MessageRefusal::Context { span, .. } if span == (9..16)
    ));
    assert!(matches!(
        refusal("MSG #ROOMS-not-a-stamp: malformed room."),
        MessageRefusal::Context { span, .. } if span == (5..22)
    ));
    assert!(matches!(
        refusal("MSG 6HmS#: empty."),
        MessageRefusal::Context { span, .. } if span == (9..9)
    ));
    assert_eq!(
        refusal("MSG #ROOMS-dVrD244Jd #ROOMS-dVrD244Jd: twice."),
        MessageRefusal::RepeatedContext {
            context: handle("ROOMS-dVrD244Jd"),
            span: 21..37,
        }
    );
    assert_eq!(
        refusal("MSG 6HmS#ROOMS-dVrD244Jd 6HmS#DESIGN-dVrD1Kz1l: one audience."),
        MessageRefusal::Repeated {
            audience: Audience::Occupant(address("6HmS")),
            span: 25..29,
        }
    );
    let prose = "MSG #ROOMS-dVrD244Jd #PLAN-dVrD245Kc 6HmS#ROOMS-dVrD244Jd: two rooms.";
    assert_eq!(
        sent(prose, &scan_messages(prose)[0]).0,
        [
            "#ROOMS-dVrD244Jd",
            "#PLAN-dVrD245Kc",
            "6HmS#ROOMS-dVrD244Jd"
        ]
    );
}

/// A hyphenless qualifier names a steering key: `#KEY` routes to the operator
/// about that direction, an occupant is never
/// addressed about a direction, and a hyphenless qualifier that spells no key
/// stays a malformed task context.
#[test]
fn a_direction_key_routes_to_the_operator_about_that_direction() {
    let key = || Some("dVwg6Zqlf".parse::<Key>().expect("fixture key"));
    let routes = |prose: &str| {
        let messages = scan_messages(prose);
        assert_eq!(messages.len(), 1, "expected one packet in {prose:?}");
        messages[0]
            .publication
            .as_ref()
            .unwrap_or_else(|refusal| panic!("refused {refusal:?} in {prose:?}"))
            .routes
            .iter()
            .map(|route| {
                (
                    route.audience,
                    route.context.clone(),
                    route.direction.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        routes("MSG #dVwg6Zqlf: Which first?"),
        [(Some(Audience::Operator), None, key())]
    );
    assert_eq!(
        routes("MSG #dVwg6Zqlf 6HmS#ROOMS-dVrD244Jd: Noted."),
        [
            (Some(Audience::Operator), None, key()),
            (
                Some(Audience::Occupant(address("6HmS"))),
                Some(handle("ROOMS-dVrD244Jd")),
                None
            ),
        ]
    );
    assert_eq!(
        refusal("MSG 6HmS#dVwg6Zqlf: wrong audience."),
        MessageRefusal::DirectionAudience { span: 4..18 }
    );
    assert_eq!(
        refusal("MSG #dVwg6Zqlf #dVwg6Zqlf: twice."),
        MessageRefusal::Repeated {
            audience: Audience::Operator,
            span: 15..25,
        }
    );
    assert!(matches!(
        refusal("MSG 6HmS#rooms: no key."),
        MessageRefusal::Context { span, .. } if span == (9..14)
    ));
    assert!(scan_messages("We tracked it in #dVwg6Zqlf: done.").is_empty());
}

/// A body keeps wrapped lines in its paragraph, but ends before a later packet
/// or a whitespace-only blank line and never captures the following report.
#[test]
fn packets_own_only_their_paragraph() {
    let prose = concat!(
        "Context the peers never see.\n\n",
        "MSG 6HmS, 7Bc 9x: First line.\n",
        "Wrapped line.\n",
        "ACK 1kJK6N3R: applied MSG 6HmS: Only for you.\n",
        " \t\r\n",
        "Operator report.",
    );
    let messages = scan_messages(prose);

    assert_eq!(messages.len(), 2);
    assert_eq!(&prose[messages[0].header.clone()], "MSG 6HmS, 7Bc 9x:");
    assert_eq!(
        sent(prose, &messages[0]),
        (vec!["6HmS", "7Bc", "9x"], "First line.\nWrapped line.")
    );
    assert_eq!(&prose[messages[1].header.clone()], "MSG 6HmS:");
    assert_eq!(sent(prose, &messages[1]), (vec!["6HmS"], "Only for you."));
    assert_eq!(answer(prose, "1kJK6N3R", &Live), Some("applied"));

    let retired: Vec<_> = scan(prose, &Live)
        .into_iter()
        .map(|recognition| recognition.outcome)
        .collect();
    assert!(matches!(
        retired.as_slice(),
        [Outcome::Retired { key, disposition: Disposition::Acked }] if key.as_str() == "1kJK6N3R"
    ));
}

/// Quoted, coded, rendered, negated, hypothetical, and narrated markers send
/// nothing, and inside a body they are content rather than boundaries.
#[test]
fn unauthored_markers_neither_publish_nor_bound_a_body() {
    for prose in [
        "> MSG 6HmS: quoted.",
        "    MSG 6HmS: indented code.",
        "```\nMSG 6HmS: fenced.\n```",
        "Use `MSG 6HmS: hi` to send.",
        "src/peer.rs:3: MSG 6HmS: rendered source.",
        "I will not MSG 6HmS: negated.",
        "If I MSG 6HmS: hypothetical.",
        "The example MSG 6HmS: narrated.",
    ] {
        assert_eq!(scan_messages(prose), [], "published {prose:?}");
    }

    let prose = concat!(
        "MSG 6HmS: Paste this:\n",
        "```\nACK 1kJK6N3R: example\nMSG 7Bc: example\n```\n",
        "> NAK 1kJK6M0V: quoted\n",
        "Then reply.",
    );
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 1);
    assert_eq!(sent(prose, &messages[0]).1, &prose["MSG 6HmS: ".len()..]);
    assert!(
        scan(prose, &Live)
            .iter()
            .all(|recognition| !matches!(recognition.outcome, Outcome::Retired { .. }))
    );
}

/// Markdown list examples describe packets; they never send or retire one.
#[test]
fn list_item_headers_open_no_packet() {
    for prose in [
        "- MSG 6HmS: unordered example.",
        "+ ACK 1kJK6N3R: unordered example.",
        "* NAK 1kJK6N3R: unordered example.",
        "1. MSG 6HmS: ordered example.",
        "  12) **ACK 1kJK6N3R:** ordered example.",
    ] {
        assert!(!has_message(prose), "published {prose:?}");
        assert!(!has_acknowledgement(prose), "retired {prose:?}");
        assert!(placed_headers(prose).is_empty(), "placed {prose:?}");
    }
}

/// A protocol name inside an ordinary clause does not become a header merely
/// because later prose contains an address-shaped token before its colon.
#[test]
fn mid_clause_message_narration_is_not_a_packet_or_body_boundary() {
    let narration = "the test must prove an ASK line inside a MSG body does not end that body, which is acceptance item 1:";
    assert!(scan_messages(narration).is_empty());
    assert!(placed_headers(narration).is_empty());

    let prose = format!("MSG 6HmS: Keep all of this: {narration}");
    let [message] = scan_messages(&prose).try_into().expect("one packet");
    assert_eq!(
        sent(&prose, &message),
        (vec!["6HmS"], &prose["MSG 6HmS: ".len()..])
    );
    assert_eq!(placed_headers(&prose), [0.."MSG 6HmS:".len()]);
}

/// Every recipient-list or body defect refuses the whole packet, decided by the
/// first defect in source order, and never borrows a neighbor's body.
#[test]
fn defective_packets_refuse_whole_without_guessing() {
    assert_eq!(
        refusal("MSG 6HmS 7Bc 6HmS: twice."),
        MessageRefusal::Repeated {
            audience: Audience::Occupant(address("6HmS")),
            span: 13..17,
        }
    );
    assert_eq!(
        refusal("MSG 6HmS 0B: leading zero."),
        MessageRefusal::Recipient {
            span: 9..11,
            error: OccupantAddressError::LeadingZero("0B".to_owned()),
        }
    );
    assert_eq!(
        refusal("MSG bob 6HmS: vowel first."),
        MessageRefusal::Recipient {
            span: 4..7,
            error: OccupantAddressError::Alphabet("bob".to_owned()),
        }
    );
    for numbered in [
        "1. MSG bob 6HmS: numbered.",
        "  12) MSG 6HmS: numbered packet",
    ] {
        assert!(scan_messages(numbered).is_empty(), "published {numbered:?}");
    }
    assert_eq!(
        refusal("MSG 6HmS please review this"),
        MessageRefusal::Unterminated
    );
    assert_eq!(refusal("MSG 6HmS, : comma."), MessageRefusal::Unterminated);
    assert_eq!(refusal("MSG 6HmS:  \n\t\n"), MessageRefusal::EmptyBody);

    let prose = "MSG 6HmS:\nACK 1kJK6N3R: done\nMSG 7Bc: hi\nMSG 9x 9x: dup";
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].publication, Err(MessageRefusal::EmptyBody));
    assert_eq!(sent(prose, &messages[1]), (vec!["7Bc"], "hi"));
    assert!(matches!(
        messages[2].publication,
        Err(MessageRefusal::Repeated { .. })
    ));
    assert_eq!(scan_acknowledgements(prose, &Live).len(), 1);
}

/// A header with no address-shaped recipient is ordinary prose using the word.
#[test]
fn prose_that_only_uses_the_word_is_not_a_packet() {
    for prose in [
        "MSG count: 3",
        "MSG: hello",
        "MSGS 6HmS: plural",
        "aMSG 6HmS: embedded",
        "MSG hello there",
        "MSG #1: first heading",
        "MSG #: hash only",
        "MSG #rooms-dVrD244Jd tag",
    ] {
        assert_eq!(scan_messages(prose), [], "published {prose:?}");
    }
}

/// An address that is also spelled like a steering key routes as an address
/// and retires nothing, even while that key is live.
#[test]
fn a_key_shaped_address_is_a_recipient_not_a_retirement() {
    let prose = "MSG dVhKT0dWx: hello";
    assert!("dVhKT0dWx".parse::<Key>().is_ok());

    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 1);
    assert_eq!(sent(prose, &messages[0]), (vec!["dVhKT0dWx"], "hello"));
    assert_eq!(scan(prose, &Live), []);
}

/// A retirement's reply owns wrapped lines in its paragraph and ends before a
/// placed packet or a blank line. An unplaced MSG marker remains reply prose.
#[test]
fn a_reply_clause_owns_only_its_paragraph() {
    let prose = "ACK 1kJK6N3R: done here MSG 6HmS: private note";
    assert_eq!(answer(prose, "1kJK6N3R", &Live), Some("done here"));
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 1);
    assert_eq!(sent(prose, &messages[0]), (vec!["6HmS"], "private note"));

    assert_eq!(
        answer("NAK 1kJK6N3R: **MSG 6HmS:** only this", "1kJK6N3R", &Live),
        None
    );
    let refused = "ACK 1kJK6N3R: done; MSG 6HmS please";
    assert_eq!(answer(refused, "1kJK6N3R", &Live), Some("done;"));
    for marker in ["ACK", "NAK"] {
        let prose = format!("{marker} 1kJK6N3R: first line\nwrapped line\n \t\r\nOperator report.");
        assert_eq!(
            answer(&prose, "1kJK6N3R", &Live),
            Some("first line\nwrapped line"),
            "reply for {prose:?}"
        );
    }
    for (prose, expected) in [
        ("ACK 1kJK6N3R: done here\nMSG 6HmS: next line", "done here"),
        (
            "ACK 1kJK6N3R: done here `MSG 6HmS: inline`",
            "done here `MSG 6HmS: inline`",
        ),
        (
            "ACK 1kJK6N3R: done here; the example MSG 6HmS: narrated",
            "done here; the example MSG 6HmS: narrated",
        ),
        (
            "ACK 1kJK6N3R: done here, MSG count: 3",
            "done here, MSG count: 3",
        ),
    ] {
        assert_eq!(answer(prose, "1kJK6N3R", &Live), Some(expected));
    }
}

/// Question and answer spellings are ordinary prose: they publish nothing,
/// refuse nothing, and stay inside the body of the packet before them.
#[test]
fn retired_question_spellings_are_prose() {
    for prose in [
        "ASK 6HmS#ROOMS-dVrD244Jd: Does caching explain it?",
        "ASK #dVwg6Zqlf: Which first?",
        "ANS dVtYchNJW#ROOMS-dVrD244Jd: It does not.",
        "ANS dVtYcnvty: Yes.",
    ] {
        assert_eq!(scan_messages(prose), [], "published {prose:?}");
    }
    let prose = "MSG 6HmS: First point.\n\
                 ASK 7Bc: still this body.\n\
                 ANS dVtYchNJW: and this.";
    let [message] = scan_messages(prose).try_into().expect("one packet");
    assert_eq!(
        sent(prose, &message),
        (
            vec!["6HmS"],
            "First point.\nASK 7Bc: still this body.\nANS dVtYchNJW: and this."
        )
    );
}
