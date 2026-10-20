//! An operator route opens an explicit pending item only for `#KEY` of a
//! direction the operator sent this lane.

use steering::{Published, Unpublished};
use wire::rooms::{RoomConversed, RoomReceiptRef};

use super::*;

impl Lanes {
    pub(super) fn conversations(&self) -> Vec<RoomConversed> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| envelope.fact.read::<RoomConversed>())
            .collect()
    }

    /// Author `text` on `lane`, account it, and hear its newest packet.
    pub(super) fn say(&self, lane: u8, text: &str) -> Result<Published, Unpublished> {
        self.author(lane, text);
        self.account(lane);
        let (_, outcomes) = self.hear_newest(lane);
        let [outcome] = outcomes.try_into().expect("one packet per block");
        outcome
    }
}

fn refused(outcome: &Result<Published, Unpublished>) -> &str {
    match outcome {
        Err(Unpublished::Refused { reason, .. } | Unpublished::AuthorDirected { reason, .. }) => {
            reason
        }
        other => panic!("the packet refused: {other:?}"),
    }
}

/// A task pointer records its room and reaches nobody: it opens no operator
/// item. Only `#KEY` of a direction the operator sent this lane opens one
/// pending operator item, beside its seated lane routes.
#[test]
fn only_a_direction_key_opens_a_pending_operator_item() {
    let lanes = Lanes::new("conv-operator");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);

    let noting = lanes.say(SENDER, &format!("MSG #{room}: status"));
    assert!(
        matches!(
            &noting,
            Ok(Published::Sent { lanes: reached, operator: false, .. }) if reached.is_empty()
        ),
        "{noting:?}"
    );
    assert_eq!(lanes.rooms().len(), 1, "the room records the packet");
    assert!(
        lanes.conversations().is_empty(),
        "a room record opens no operator item"
    );

    let own = lanes.direct(SENDER, 60, "ship when green");
    let telling = lanes.say(SENDER, &format!("MSG #{own} {first}#{room}: shipped"));
    assert!(
        matches!(
            &telling,
            Ok(Published::Sent { lanes: reached, operator: true, .. })
                if reached == &[test_uuid(FIRST)]
        ),
        "{telling:?}"
    );
    let [told] = lanes
        .conversations()
        .try_into()
        .expect("one conversation record");
    let item = RoomReceiptRef::new(told.publication);
    let opened = lanes
        .bus
        .read(move |read| read.room_item(&item))
        .expect("items read");
    assert!(
        matches!(
            opened.as_ref().map(|(_, head)| head.body().state()),
            Some(wire::rooms::RoomReceiptState::Pending { .. })
        ),
        "the direction route opens one pending item: {opened:?}"
    );
    lanes.finish();
}

/// `#KEY` routes to the operator about a direction the operator sent this
/// lane; another lane's direction, a peer message's key, an unknown key, an
/// occupant addressed about a direction, or the context-free legacy spelling
/// publishes nothing.
#[test]
fn an_operator_concern_names_the_direction_it_points_at() {
    let lanes = Lanes::new("conv-direction");
    let (sender, first) = (lanes.address(SENDER), lanes.address(FIRST));
    let bare = lanes.say(SENDER, "MSG operator: status");
    let Err(Unpublished::Refused { reason, detail }) = &bare else {
        panic!("the context-free legacy spelling refuses: {bare:?}");
    };
    assert_eq!(reason, "msg::concern-missing");
    assert!(detail.contains("#KEY"), "{detail}");
    assert!(detail.contains("#TASK"), "{detail}");
    assert!(!detail.contains("operator#"), "{detail}");

    let own = lanes.direct(SENDER, 60, "which first");
    let theirs = lanes.direct(FIRST, 62, "hold the merge");
    assert!(
        lanes
            .say(FIRST, &format!("MSG {sender}: peer note"))
            .is_ok()
    );
    let peer = lanes
        .pending(SENDER)
        .into_iter()
        .map(|pending| pending.key)
        .find(|key| *key != own)
        .expect("the peer message is pending");
    for (selector, reason) in [
        (format!("#{theirs}"), "msg::direction-other-lane"),
        (format!("#{peer}"), "msg::direction-not-operator"),
        ("#dVwg6Zqlf".to_owned(), "msg::direction-unknown"),
        (format!("{first}#{own}"), "msg::direction-audience"),
        ("operator".to_owned(), "msg::concern-missing"),
    ] {
        let outcome = lanes.say(SENDER, &format!("MSG {selector}: about it"));
        assert_eq!(refused(&outcome), reason, "{selector}");
    }
    assert!(lanes.conversations().is_empty());

    lanes
        .say(SENDER, &format!("MSG #{own}: which first?"))
        .expect("the concern publishes");
    let directions = lanes
        .conversations()
        .into_iter()
        .flat_map(|record| record.routes)
        .map(|route| route.direction)
        .collect::<Vec<_>>();
    assert_eq!(directions, [Some(own)]);
    lanes.finish();
}

/// The operator reads a lane's operator MSG body from the author's
/// transcript, the bytes `spice room inbox` prints, and an unknown publication
/// reads as missing rather than as some other text.
#[test]
fn an_operator_item_body_reads_back_from_its_authoring_transcript() {
    let lanes = Lanes::new("conv-operator-text");
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key}: ship it?"))
        .expect("the concern publishes");
    let [concern] = lanes
        .conversations()
        .try_into()
        .expect("one conversation record");
    let read = |publication| {
        let outcome = crate::room_command::conversation_text(
            &lanes.bus,
            &lanes.paths.tape,
            &wire::intent::RoomConversationText { publication },
        );
        let wire::Outcome::Reply(reply) = outcome else {
            panic!("the conversation text read answers: {outcome:?}");
        };
        let wire::intent::CommandAnswer::RoomConversationText(answer) = reply.body else {
            panic!("another answer shape: {:?}", reply.body);
        };
        answer.content
    };
    assert_eq!(
        read(concern.publication.clone()),
        wire::intent::RoomConversationContent::Text {
            text: "ship it?".to_owned()
        }
    );
    assert_eq!(
        read("dVrD244Jz".parse().expect("fixture publication")),
        wire::intent::RoomConversationContent::Unavailable {
            unavailable: wire::rooms::RoomSourceUnavailable::Missing {}
        }
    );
    lanes.finish();
}
