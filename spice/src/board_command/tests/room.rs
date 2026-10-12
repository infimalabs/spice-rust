use super::*;

use wire::rooms::{
    RoomAuthorityStamp, RoomListenerAvailability, RoomListenerDelivery, RoomPost, RoomPostOutcome,
    RoomPublicationContent, RoomPublished,
};

fn publication_page(bus: &Bus, room: &TaskHandle) -> journal::SelectedFactPage {
    let room = room.born().expect("fixture room has a birth stamp");
    bus.read(move |read| {
        read.room_publications(
            &room,
            None,
            journal::FactReadBudget::new(32, journal::MAX_RETAINED_FACT_PAGE_BYTES)
                .expect("fixture room page budget"),
        )
    })
    .expect("room publication page reads")
}

fn publish_listener(bus: &Bus, room: &TaskHandle, lane: CanonicalUuid, micros: i64) {
    let room = room.born().expect("fixture room has a birth stamp");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let origin = format!("task:{room}").parse().expect("fixture task origin");
    let mut write = JournalWrite::new(Actor::Lane(lane), origin, at, Producer::CommandDoor);
    write
        .fact(
            RoomPublished {
                room,
                publication: RoomAuthorityStamp::for_micros(micros)
                    .expect("fixture publication identity"),
                content: RoomPublicationContent::Authored {
                    text: "substantive listener evidence".to_owned(),
                },
            },
            FactCause::Root,
        )
        .expect("fixture room publication builds");
    assert!(matches!(
        bus.commit(write)
            .expect("fixture publication reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn vacate(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) {
    let observed = bus
        .task_snapshot()
        .expect("fixture residency snapshot reads")
        .residency()
        .expect("fixture residency exists")
        .clone();
    let mut vacant = observed.body().clone();
    let row = vacant
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane exists");
    row.occupant = None;
    row.disposition = "vacant".to_owned();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    let exited = write
        .fact(
            wire::body::LaneExited {
                lane: seat.lane,
                occupant: seat.occupant,
                code: 0,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("fixture exit builds");
    write.residency_head(exited, move |_| vacant);
    assert!(matches!(
        bus.commit(write).expect("fixture exit reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn post_command(
    bus: &Bus,
    epoch: CanonicalUuid,
    room: &TaskHandle,
    text: &str,
) -> wire::AdmittedCommand {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, room);
    fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![statement_witness(heads)],
        &RoomPost {
            room: room.clone(),
            text: text.to_owned(),
        },
    )
}

#[test]
fn room_post_records_once_and_retains_its_exact_retry_answer() {
    let (_root, bus, epoch, _seats) = fixture("room-post-retry", 0);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-retry",
        "Room",
        None,
    );
    let command = post_command(&bus, epoch, &room, "Complete composer text.");
    let first = dispatch(&bus, &command).expect("room command dispatches");
    let answer = assert_committed(first.clone());
    let CommandAnswer::RoomPost(answer) = answer else {
        panic!("room post returned another answer shape: {answer:?}");
    };
    assert!(matches!(
        answer.outcome,
        RoomPostOutcome::RecordedNoCurrentListener { .. }
    ));

    let replay_command = command.clone();
    let replay = bus
        .read(move |read| read.command_replay(&replay_command))
        .expect("command retry receipt reads");
    assert_eq!(replay, Some(journal::CommandReplay::Exact(first)));

    let page = publication_page(&bus, &room);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.continuation, None);
    assert_eq!(page.rows[0].actor, Actor::Operator);
    let recorded = page.rows[0]
        .fact
        .read::<RoomPublished>()
        .expect("indexed publication has its typed body");
    assert_eq!(recorded.publication, answer.publication);
    assert_eq!(
        recorded.content,
        RoomPublicationContent::Authored {
            text: "Complete composer text.".to_owned()
        }
    );
}

#[test]
fn room_post_reports_occupied_vacant_and_unknown_listeners_without_waking() {
    let (_root, bus, epoch, seats) = fixture("room-post-listeners", 2);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-listeners",
        "Room",
        None,
    );
    let micros = jiff::Timestamp::now().as_microsecond();
    publish_listener(&bus, &room, seats[0].lane, micros);
    publish_listener(&bus, &room, seats[1].lane, micros + 1);
    let unknown = uuid(900_000);
    publish_listener(&bus, &room, unknown, micros + 2);
    vacate(&bus, epoch, &seats[1]);

    let answer = assert_committed(
        dispatch(&bus, &post_command(&bus, epoch, &room, "Recorded only."))
            .expect("room command dispatches"),
    );
    let CommandAnswer::RoomPost(answer) = answer else {
        panic!("room post returned another answer shape: {answer:?}");
    };
    let RoomPostOutcome::RecordedWithListeners { listeners } = answer.outcome else {
        panic!("current listeners were not reported")
    };
    assert_eq!(listeners.len(), 3);
    let occupied = listeners
        .iter()
        .find(|listener| listener.lane == seats[0].lane)
        .expect("occupied listener is present");
    assert_eq!(occupied.availability, RoomListenerAvailability::Occupied);
    assert_eq!(occupied.delivery, RoomListenerDelivery::AtSafeBoundary);
    let vacant = listeners
        .iter()
        .find(|listener| listener.lane == seats[1].lane)
        .expect("vacant listener is present");
    assert_eq!(vacant.availability, RoomListenerAvailability::Vacant);
    assert_eq!(vacant.delivery, RoomListenerDelivery::WaitsForNextStart);
    let unknown = listeners
        .iter()
        .find(|listener| listener.lane == unknown)
        .expect("unknown listener is present");
    assert_eq!(unknown.availability, RoomListenerAvailability::Unknown);
    assert_eq!(unknown.delivery, RoomListenerDelivery::PendingAvailability);
    assert!(
        bus.task_snapshot()
            .expect("post-residency reads")
            .residency()
            .expect("post-residency exists")
            .body()
            .lanes
            .iter()
            .any(|row| row.lane == seats[1].lane && row.occupant.is_none())
    );
}

#[test]
fn room_post_refuses_unknown_anchor_without_recording() {
    let (_root, bus, epoch, _seats) = fixture("room-post-unknown", 0);
    let unknown_stamp = stamp("room-post-unknown-anchor");
    let unknown = TaskHandle::for_project("board.test", &unknown_stamp);
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![Witness::Task {
            task: unknown_stamp,
            head: Seq::new(1).expect("fixture position"),
        }],
        &RoomPost {
            room: unknown.clone(),
            text: "Do not record.".to_owned(),
        },
    );
    let outcome = dispatch(&bus, &command).expect("room command dispatches");
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "room::anchor-unknown"
    ));
    assert!(publication_page(&bus, &unknown).rows.is_empty());
}
