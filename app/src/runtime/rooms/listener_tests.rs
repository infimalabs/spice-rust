use wire::rooms::{RoomAuthorityStamp, RoomContributionKind};
use wire::{CanonicalUuid, Seq};

use super::{ConversationProjection, RoomEvent, RoomEventKind, RoomState};

fn lane(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
        .parse()
        .expect("fixture lane")
}

fn publication(micros: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(micros).expect("fixture publication")
}

fn position(value: u64) -> Seq {
    Seq::new(value).expect("fixture position")
}
fn contribution(lane: CanonicalUuid, sequence: u64) -> RoomEvent {
    RoomEvent {
        position: position(sequence),
        kind: RoomEventKind::Contributed {
            lane,
            contribution: RoomContributionKind::Evidence,
            text: "evidence".to_owned(),
            sources: Vec::new(),
        },
    }
}

#[test]
fn contributions_never_mark_listener_joins() {
    let first = lane(1);
    let second = lane(2);
    let first_entry = publication(1_700_000_000_000_001);
    let repeated = publication(1_700_000_000_000_002);
    let left = publication(1_700_000_000_000_003);
    let reentry = publication(1_700_000_000_000_004);
    let second_entry = publication(1_700_000_000_000_005);
    let mut room = RoomState::default();
    room.events.insert(first_entry, contribution(first, 1));
    room.events.insert(repeated, contribution(first, 2));
    room.events.insert(
        left,
        RoomEvent {
            position: position(3),
            kind: RoomEventKind::Left {
                lane: first,
                clause: "handoff".to_owned(),
            },
        },
    );
    room.events.insert(reentry, contribution(first, 4));
    room.events.insert(second_entry, contribution(second, 5));

    assert!(
        room.listener_joins(&ConversationProjection::default())
            .is_empty(),
        "participation does not establish a listening interval",
    );
}
