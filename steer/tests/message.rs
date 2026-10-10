//! Reading authored MSG packets out of assistant prose.
//!
//! MSG shares the acknowledgment header grammar's authored-context rules but
//! publishes rather than retires: each placed packet names occupant addresses
//! and owns exactly the body hunk up to the next placed packet.

use steer::ack::keys::Key;
use steer::ack::scan::{
    Message, MessageRefusal, Outcome, Standing, scan, scan_acknowledgements, scan_messages,
};
use wire::{Disposition, OccupantAddress, OccupantAddressError};

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

/// Recipients as written and the exact body bytes one packet publishes.
fn sent<'prose>(prose: &'prose str, message: &Message) -> (Vec<&'prose str>, &'prose str) {
    let publication = message
        .publication
        .as_ref()
        .unwrap_or_else(|refusal| panic!("refused {refusal:?} in {prose:?}"));
    let recipients = publication
        .recipients
        .iter()
        .map(|recipient| {
            let written = &prose[recipient.start..recipient.end];
            assert_eq!(recipient.address, address(written));
            written
        })
        .collect();
    (recipients, &prose[publication.body.clone()])
}

fn refusal(prose: &str) -> MessageRefusal {
    let messages = scan_messages(prose);
    assert_eq!(messages.len(), 1, "expected one packet in {prose:?}");
    messages[0]
        .publication
        .clone()
        .expect_err("packet is refused")
}

/// A body runs from its colon to the next placed packet, keeping its own bytes
/// and none of the prose before the first header or the framing of the next.
#[test]
fn consecutive_packets_own_exact_multiline_bodies() {
    let prose = concat!(
        "Context the peers never see.\n\n",
        "MSG 6HmS, 7Bc 9x: First line.\n\n",
        "Second paragraph keeps its\n  indentation.\n",
        "- ACK 1kJK6N3R: applied\n",
        "- **MSG 6HmS:** Only for you.\n",
    );
    let messages = scan_messages(prose);

    assert_eq!(messages.len(), 2);
    assert_eq!(&prose[messages[0].header.clone()], "MSG 6HmS, 7Bc 9x:");
    assert_eq!(
        sent(prose, &messages[0]),
        (
            vec!["6HmS", "7Bc", "9x"],
            "First line.\n\nSecond paragraph keeps its\n  indentation."
        )
    );
    assert_eq!(&prose[messages[1].header.clone()], "**MSG 6HmS:**");
    assert_eq!(sent(prose, &messages[1]), (vec!["6HmS"], "Only for you."));

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

/// Every recipient-list or body defect refuses the whole packet, decided by the
/// first defect in source order, and never borrows a neighbor's body.
#[test]
fn defective_packets_refuse_whole_without_guessing() {
    assert_eq!(
        refusal("MSG 6HmS 7Bc 6HmS: twice."),
        MessageRefusal::Repeated {
            address: address("6HmS"),
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
    assert_eq!(
        refusal("MSG 6HmS please review this"),
        MessageRefusal::Unterminated
    );
    assert_eq!(refusal("MSG 6HmS, : comma."), MessageRefusal::Unterminated);
    assert_eq!(refusal("MSG 6HmS:  \n\t\n"), MessageRefusal::EmptyBody);

    let prose = "MSG 6HmS:\n- ACK 1kJK6N3R: done\nMSG 7Bc: hi\nMSG 9x 9x: dup";
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
