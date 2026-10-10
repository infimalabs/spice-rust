use super::*;

fn author() -> CanonicalUuid {
    "018f82d4-6f59-7d06-92e3-bce9131163f1"
        .parse()
        .expect("fixture author parses")
}

fn recipient(address: &str, lane: &str, occupant: &str) -> wire::MessageRecipient {
    wire::MessageRecipient {
        address: address.parse().expect("fixture address parses"),
        lane: lane.parse().expect("fixture lane parses"),
        occupant: occupant.parse().expect("fixture occupant parses"),
    }
}

fn reviewers() -> [wire::MessageRecipient; 2] {
    [
        recipient(
            "6K26",
            "018f82d4-6f59-7d06-92e3-bce9131163a1",
            "018f82d4-6f59-7d06-92e3-bce9131163a2",
        ),
        recipient(
            "6Jv5",
            "018f82d4-6f59-7d06-92e3-bce9131163b1",
            "018f82d4-6f59-7d06-92e3-bce9131163b2",
        ),
    ]
}

fn proved(recipients: &[wire::MessageRecipient]) -> PresentationDirectives {
    let mut ledger = PresentationDirectives::default();
    for recipient in recipients {
        ledger.recipients.insert(recipient.address, *recipient);
    }
    ledger
}

fn authored() -> PresentationEvent {
    PresentationEvent {
        author: Some(author()),
        ..event()
    }
}

fn present(
    text: &str,
    ledger: &PresentationDirectives,
    event: PresentationEvent,
) -> serve::stream::MessagePresentation {
    present_steering_acknowledgements(text, Some(ledger), event).expect("message presents")
}

#[test]
fn an_addressed_packet_presents_its_body_as_one_outgoing_quote_in_header_order() {
    let [first, second] = reviewers();
    let presented = present(
        "Asking both reviewers.\nMSG 6Jv5 6K26: Please check the parser.\nIt rejects empty input.",
        &proved(&[first, second]),
        authored(),
    );

    assert_eq!(
        presented.text,
        "Asking both reviewers.\n> Please check the parser.\n> It rejects empty input."
    );
    let [send] = presented.sends.as_slice() else {
        panic!("one packet presents one send: {:?}", presented.sends)
    };
    assert_eq!(
        &presented.text[send.start as usize..send.end as usize],
        "> Please check the parser.\n> It rejects empty input."
    );
    assert_eq!((send.from, send.sender), (lane(), author()));
    assert_eq!(send.recipients, vec![second, first]);
}

#[test]
fn a_packet_and_a_following_acknowledgement_present_as_separate_quotes() {
    let key = "1kQnrzNZ";
    let directive = "Keep one tested path.";
    let mut ledger = proved(&reviewers());
    ledger.directives.insert(
        key.parse().expect("fixture key parses"),
        PresentationDirective {
            actor: wire::Actor::Operator,
            sender: None,
            published: "2030-04-12T18:44:09.001Z"
                .parse()
                .expect("fixture instant parses"),
            lane: lane(),
            text: directive.to_owned(),
            authored: true,
            taste: false,
            review: false,
            disposition: wire::Disposition::Live,
            quote_target: wire::QuoteTarget::Steering {
                key: key.to_owned(),
                digest: wire::QuoteTarget::text_digest(directive),
            },
            retirement: None,
        },
    );

    let presented = present(
        "MSG 6Jv5: Ship it.\nACK 1kQnrzNZ: done.",
        &ledger,
        authored(),
    );

    assert_eq!(
        presented.text,
        "> Ship it.\n\n> Keep one tested path.\n\nDone."
    );
    let ([send], [context]) = (
        presented.sends.as_slice(),
        presented.directive_contexts.as_slice(),
    ) else {
        panic!("one send and one directive context present");
    };
    assert_eq!(
        &presented.text[send.start as usize..send.end as usize],
        "> Ship it."
    );
    assert_eq!(
        &presented.text[context.start as usize..context.end as usize],
        "> Keep one tested path."
    );
}

#[test]
fn a_packet_without_proved_recipients_or_an_authoring_occupant_stays_literal() {
    let text = "MSG 6Jv5 6K26: Please check the parser.";
    let [first, _] = reviewers();

    for (ledger, event) in [
        // One unproved address withholds the whole packet's presentation.
        (proved(&[first]), authored()),
        // A tool caption names no authoring occupant and never sends.
        (proved(&reviewers()), event()),
    ] {
        let presented = present(text, &ledger, event);
        assert_eq!(presented.text, text);
        assert!(presented.sends.is_empty());
    }
}
