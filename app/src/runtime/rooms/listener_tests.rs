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
fn event_authority_marks_first_entry_and_reentry_without_replay_duplicates() {
    let first = lane(1);
    let second = lane(2);
    let first_entry = publication(1_700_000_000_000_001);
    let repeated = publication(1_700_000_000_000_002);
    let left = publication(1_700_000_000_000_003);
    let reentry = publication(1_700_000_000_000_004);
    let second_entry = publication(1_700_000_000_000_005);
    let mut room = RoomState::default();
    room.events
        .insert(first_entry.clone(), contribution(first, 1));
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
    room.events.insert(reentry.clone(), contribution(first, 4));
    room.events
        .insert(second_entry.clone(), contribution(second, 5));

    let joins = room.listener_joins(&ConversationProjection::default());
    assert_eq!(
        joins.into_iter().collect::<Vec<_>>(),
        vec![
            (&first_entry, vec![first]),
            (&reentry, vec![first]),
            (&second_entry, vec![second]),
        ],
        "only the first exact event in each listening interval owns a join",
    );
}
